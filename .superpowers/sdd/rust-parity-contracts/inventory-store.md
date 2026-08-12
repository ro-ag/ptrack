# Inventory: Storage layer — internal/store (Go v0.21.0)

Scope: `internal/store/*.go` plus the persisted struct definitions in
`internal/model/model.go` (they are the gob payloads and therefore part of the
on-disk contract) and the bbolt dependency (`go.etcd.io/bbolt v1.5.0`,
`go.mod:14`).

## Contracts

### Database files & locations

- STORE-001: Project database lives at `<projectRoot>/.ptrack/ptrack.db` (dir `.ptrack`, file `ptrack.db`). — src: internal/store/discovery.go:13-17 — pinned by: internal/store/discovery_test.go/TestInitAndFindSameDir — verify: fixture
- STORE-002: Global database lives at `$PTRACK_HOME/global.db` when `PTRACK_HOME` is set and non-empty, else `~/.ptrack/global.db`. The home dir is created with mode 0o755 on open. — src: internal/store/global.go:26-48 — pinned by: internal/store/global_test.go/openGlobalTemp (uses PTRACK_HOME) — verify: fixture
- STORE-003: Both databases are bbolt (go.etcd.io/bbolt v1.5.0) single-file databases, opened with file mode 0o600. The Rust rewrite must either read/write the bbolt file format byte-compatibly or ship a one-way migration; coexistence with the Go binary requires the bbolt format. — src: internal/store/store.go:66, internal/store/global.go:48, go.mod:14 — pinned by: none — verify: fixture (open Go-written DB with Rust reader)
- STORE-004: Both DBs are opened with `bolt.Options{Timeout: time.Second}`: bbolt takes an exclusive `flock` on the file; a second process opening the same DB blocks up to 1s then fails. There is no inter-process shared access; concurrency within a process is bbolt's single-writer/multi-reader MVCC. — src: internal/store/store.go:66, internal/store/global.go:48 — pinned by: none — verify: manual (two processes, second open times out)
- STORE-005: `FindProjectDB(start)` walks up from `start` (absolutized) returning the first existing `<dir>/.ptrack/ptrack.db` that is not a directory; it stops with `ErrNoProject` ("no ptrack project found (run 'ptrack init')") after inspecting a directory containing a `.git` marker (directory OR regular file — covers git worktrees) without a project DB, and at the filesystem root. — src: internal/store/discovery.go:23-44,94-97 — pinned by: TestFindFromNestedSubdir, TestFindNoProject, TestFindStopsAtGitBoundary, TestFindStopsAtGitWorktreeFileBoundary — verify: automated test
- STORE-006: `InitProject(dir)` errors with "ptrack project already exists at <path>" if `<dir>/.ptrack/ptrack.db` exists; otherwise creates `.ptrack` (0o755) and returns the DB path WITHOUT creating the file (file is created by the subsequent `Open`). Empty `dir` resolves to the enclosing git root if any, else the cwd. — src: internal/store/discovery.go:49-76 — pinned by: TestInitTwiceFails — verify: automated test
- STORE-007: `Store.ProjectRoot()` resolves the DB path through Abs+EvalSymlinks and requires the parent dir basename to be exactly `.ptrack`, else errors ("project database must be inside a .ptrack directory" / "project store is required" on nil). — src: internal/store/store.go:84-101 — pinned by: none — verify: automated test

### Project DB bucket schema (`ptrack.db`)

All buckets are top-level bbolt buckets created with `CreateBucketIfNotExists` on
every open (internal/store/store.go:103-109). Bucket names are the literal byte
strings below.

- STORE-010: Bucket `meta` holds exactly one key `meta` (bytes "meta") whose value is the gob-encoded `model.Meta` singleton. — src: internal/store/store.go:45,55,110-120 — pinned by: internal/store/store_test.go/TestMetaLifecycle — verify: fixture
- STORE-011: Buckets `plans`, `tasks`, `milestones`, `issues`, `commits`, `capabilities`, `capability_audits` are keyed by 8-byte big-endian uint64 ids (`itob`), ids allocated via bbolt `Bucket.NextSequence()` (per-bucket monotonic counter starting at 1, persisted in the bucket). — src: internal/store/codec.go:23-29, internal/store/store.go:44-54,207,288 — pinned by: store_test.go/TestPlanCRUD, milestones_test.go/TestMilestoneCRUDAndPlanLink — verify: fixture
- STORE-012: Bucket `notes` uses the same big-endian-uint64 NextSequence keys; because ids are big-endian, bbolt cursor order == numeric id order == insertion order (used implicitly by RecentNotes and bounded scans). — src: internal/store/store.go:538-548, internal/store/bounded.go:188 — pinned by: store_test.go/TestNotes — verify: fixture
- STORE-013: Bucket `memory_writebacks` is keyed by the raw UTF-8 request-ID string (not itob); values are gob-encoded internal `memoryWritebackRecord{Digest [32]byte, Sequence uint64, Kind model.MemoryKind, NoteID uint64}`. The gob type name on the wire is "memoryWritebackRecord" (unexported type, package `store`). — src: internal/store/memory_writeback.go:54-59,75,134 — pinned by: memory_writeback_test.go/TestWriteMemoryReplayReceiptsAreBounded — verify: fixture
- STORE-014: Complete project-DB bucket list (10): `meta`, `plans`, `tasks`, `notes`, `milestones`, `issues`, `commits`, `capabilities`, `capability_audits`, `memory_writebacks`. All 10 are created on every `Open`, even when opening an older DB rejected for version (rollback discards them — see STORE-021). — src: internal/store/store.go:44-56,103-109 — pinned by: version_test.go/TestMigrateV4AddsWritebackReceiptsAndPreservesLegacyNotes — verify: fixture

### Value encodings (gob payloads)

All struct values are encoded with Go `encoding/gob` (self-describing: each
value carries a type descriptor with the Go type name and field names; only
non-zero fields are transmitted; `time.Time` uses its GobEncode binary form —
wall clock seconds+nanoseconds+zone offset, no monotonic clock). Unknown/missing
fields decode to zero values, so adding a struct field is forward/backward
compatible within gob. — src: internal/store/codec.go:9-21, internal/model/model.go:1-4 — pinned by: internal/model/model_test.go/TestGobRoundTrip — verify: fixture (golden gob bytes per type)

- STORE-020: `model.Meta` gob shape: `Goal string; Summary string; ActivePlan uint64; CreatedAt time.Time; UpdatedAt time.Time; FormatVersion uint; LastWriteVersion string`. FormatVersion 0 means a pre-versioning (v0.1.0) DB, adopted as v1. LastWriteVersion is diagnostic only, never gates behavior. — src: internal/model/model.go:96-111 — pinned by: TestGobRoundTrip, version_test.go/TestAdoptPreVersioningDB — verify: fixture
- STORE-021: `model.Plan` gob shape: `ID uint64; Title string; Status PlanStatus("active"|"done"|"archived"); MilestoneID uint64 (0=unassigned); Order int; CreatedAt, UpdatedAt time.Time`. — src: internal/model/model.go:127-135,33-39 — pinned by: TestGobRoundTrip — verify: fixture
- STORE-022: `model.Task` gob shape: `ID; PlanID uint64; Title string; Status TaskStatus("todo"|"doing"|"done"|"blocked"); Order int; CreatedAt; UpdatedAt`. — src: internal/model/model.go:161-169,41-48 — pinned by: TestGobRoundTrip — verify: fixture
- STORE-023: `model.Note` gob shape: `ID; Target NoteTarget("project"|"plan"|"task"); TargetID uint64; Kind MemoryKind("" for legacy notes | "decision"|"blocker"|"handoff"; "summary" is NEVER stored as a Note.Kind); Body string; CreatedAt`. — src: internal/model/model.go:173-180,50-65 — pinned by: memory_writeback_test.go/TestLegacyNotesDecodeWithoutTypedKind — verify: fixture
- STORE-024: `model.Milestone` gob shape: `ID; Title; Status MilestoneStatus("open"|"done"); Due time.Time (zero = none); Order int; CreatedAt; UpdatedAt`. — src: internal/model/model.go:115-123,67-70 — pinned by: none (gob round trip indirect) — verify: fixture
- STORE-025: `model.Issue` gob shape: `ID; Title; Body; Status IssueStatus("open"|"closed"); Severity Severity("low"|"medium"|"high"|"critical"); TaskID uint64 (0=unlinked); CreatedAt; UpdatedAt`. — src: internal/model/model.go:149-158,72-81 — pinned by: milestones_test.go/TestIssueCRUD — verify: fixture
- STORE-026: `model.Commit` gob shape: `ID; SHA string; Subject string; PlanID uint64; TaskID uint64; CreatedAt`. — src: internal/model/model.go:139-146 — pinned by: commits_test.go/TestCommitAddDedupAndLink — verify: fixture
- STORE-027: `model.Capability` gob shape: `ID; ModelVersion uint (current 1); Revision uint64; Name; Kind CapabilityKind("http"|"git"|"ssh"); AgentProfile string; Enabled bool; ApprovalDurationSeconds int64; ApprovedAt; ExpiresAt; ScopeDigest string; Limits CapabilityLimits{TimeoutSeconds int; MaxRequestBytes, MaxResponseBytes, MaxOutputBytes int64; MaxRedirects, MaxConcurrent int}; Audit CapabilityAuditPolicy{Enabled bool; RetainLast int}; HTTP *HTTPScope{BaseURL; Methods []string; PathPrefixes []string}; Git *GitScope{RemoteName; RemoteURL; Operations, Branches, Refspecs []string; AllowTags, AllowForcePush, AllowDeleteRefs bool}; SSH *SSHScope{Alias; Host; Port uint16; User; HostKey; AllowGit bool; RemoteCommands []string; AllowUpload, AllowDownload bool; UploadRoots, DownloadRoots, UploadRemoteRoots, DownloadRemoteRoots []string; AllowInteractiveShell bool; LocalForwardTargets, RemoteForwardTargets []string}; CreatedAt; UpdatedAt`. JSON tags exist but the on-disk encoding is gob (field names, not json tags). — src: internal/model/model.go:182-269,91-94 — pinned by: TestGobRoundTrip (Capability incl. Git scope) — verify: fixture
- STORE-028: `model.CapabilityAudit` gob shape: `ID; CapabilityID uint64; AgentProfile; Kind; Operation; Target string; Success bool; ErrorClass string; DurationMillis, RequestBytes, ResponseBytes int64; Redirects int; CreatedAt`. Metadata-only by design: no bodies/headers/credentials persisted. — src: internal/model/model.go:271-288 — pinned by: capabilities_test.go/TestCapabilityLifecycleAndAuditRetention — verify: fixture
- STORE-029: Global DB bucket `projects`: key = absolute project path as raw bytes; value = gob `model.ProjectRef{Name string; Path string (absolute); LastSeen time.Time}`. — src: internal/store/global.go:14-18,90-104 — pinned by: global_test.go/TestProjectRegistry — verify: fixture
- STORE-030: Global DB bucket `config`: plain string→string key/value, raw bytes, no encoding. Missing key reads as "". — src: internal/store/global.go:71-88 — pinned by: global_test.go/TestConfigSetGet — verify: fixture
- STORE-031: Global DB bucket `backups`: key = decimal string of `time.Now().UnixNano()`; value = raw bytes `"<projectPath>\t<backupPath>"` (tab-separated, NOT gob). — src: internal/store/global.go:161-167 — pinned by: none — verify: fixture

### Schema versioning & migration

- STORE-040: `CurrentFormat = 5`. v2 added milestones/issues; v3 added commits; v4 added capabilities + capability_audits; v5 added memory_writebacks + typed Note.Kind. A fresh DB's meta record is stamped FormatVersion=5, LastWriteVersion=WriterVersion, CreatedAt=UpdatedAt=now at open time. — src: internal/store/store.go:23-31,110-120 — pinned by: version_test.go/TestFreshDBStampsCurrentFormat — verify: automated test
- STORE-041: Opening a DB with FormatVersion > CurrentFormat fails with `ErrFormatTooNew` whose message is exactly `database format v%d is newer than this ptrack (supports v%d) — upgrade ptrack` (note the em dash). The transaction is rolled back so the newer DB is left byte-untouched. — src: internal/store/store.go:33-42,125-129 — pinned by: version_test.go/TestRejectNewerFormat — verify: automated test
- STORE-042: Opening a DB with FormatVersion < CurrentFormat (including 0) silently migrates in place: bumps FormatVersion to CurrentFormat, sets LastWriteVersion and UpdatedAt; all data migration is shape-only (new buckets created by init; new gob fields decode as zero). No per-version transforms exist. — src: internal/store/store.go:130-154 — pinned by: version_test.go/TestAdoptPreVersioningDB, milestones_test.go/TestV1DBMigratesToV2, commits_test.go/TestV2DBMigratesToV3, version_test.go/TestMigrateV4AddsWritebackReceiptsAndPreservesLegacyNotes — verify: automated test
- STORE-043: `WriterVersion` is a process-global set once by main from the CLI semver (default "dev"), written into Meta.LastWriteVersion on every meta mutation. — src: internal/store/store.go:29-31,176, main.go:24 — pinned by: version_test.go/TestFreshDBStampsCurrentFormat — verify: automated test

### Write semantics / observable mutation behavior

- STORE-050: Every mutation runs in a single bbolt read-write transaction; failure rolls back atomically (bbolt guarantee, relied on e.g. by WriteMemory target validation and ConvertTaskToPlan). There is no WAL/sidecar; crash recovery is bbolt's own meta-page double-buffering/freelist behavior. — src: internal/store/store.go:104,168,205 etc. — pinned by: memory_writeback_test.go/TestWriteMemoryIdempotencyCollisionAndTargetValidationAreAtomic — verify: automated test
- STORE-051: New plans/tasks/milestones get `Order = bucket.Stats().KeyN` at insert (key count BEFORE insert), Status active/todo/open respectively, CreatedAt=UpdatedAt=now (local time, `time.Now()` — not UTC except via WriteMemory which uses `.UTC()`). List functions sort by Order ascending with a stable sort, not by id. — src: internal/store/store.go:203-240,281-302, internal/store/milestones.go:13-50, internal/store/helpers.go:40-43 — pinned by: store_test.go/TestPlanCRUD, TestTaskCRUD — verify: automated test
- STORE-052: `AddTask`, `SetTaskPlan`, `SetActivePlan`, `SetPlanMilestone`, `AddIssue(taskID)` validate the referenced entity exists and return sentinel `ErrNotFound` ("not found") otherwise. GetX of a missing id also returns ErrNotFound. — src: internal/store/store.go:16-17,193-198,284,398, internal/store/milestones.go:97-113,140 — pinned by: TestActivePlanRequiresExisting, TestSetTaskPlan, TestIssueCRUD — verify: automated test
- STORE-053: All mutate paths bump `UpdatedAt = time.Now()` (local). `updateMeta` additionally stamps LastWriteVersion. — src: internal/store/store.go:167-179,273,527, internal/store/milestones.go:88,213 — pinned by: TestMetaLifecycle — verify: automated test
- STORE-054: `AddCommit` is idempotent by SHA: a linear scan over the commits bucket finds an existing record with equal SHA and returns it unchanged (no new id consumed). — src: internal/store/commits.go:10-46 — pinned by: commits_test.go/TestCommitAddDedupAndLink — verify: automated test
- STORE-055: `CompareAndSetTaskStatus` is a CAS fence: fails with error wrapping `ErrTaskStatusChanged` ("task status changed") and message `"task status changed: task #%d is plan #%d/%q at %s, expected plan #%d/%q at %s"` (times RFC3339Nano UTC) when plan id, status, or UpdatedAt (exact `time.Time.Equal`) differ; a no-op when current status already equals the target. — src: internal/store/store.go:19-21,353-387 — pinned by: store_test.go/TestCompareAndSetTaskStatusFencesPlanAndStatus — verify: automated test
- STORE-056: `ConvertTaskToPlan` in one transaction: new plan gets task's Title and CreatedAt, parent plan's MilestoneID, status done iff task was done else active; task's notes re-target to the plan; task's commits get TaskID=0/PlanID=newPlan; linked issues are unlinked (TaskID=0, UpdatedAt bumped); the task row is deleted. — src: internal/store/store.go:412-517 — pinned by: task_conversion_test.go/TestConvertTaskToPlanPreservesRelatedData, TestConvertDoneTaskCreatesDonePlan, TestConvertMissingTaskDoesNotCreatePlan — verify: automated test
- STORE-057: `AddIssue` defaults empty severity to "medium"; `AddCapability` defaults ModelVersion to 1 when 0 and forces Revision=1; `UpdateCapability` increments Revision, preserves ID/ModelVersion/CreatedAt, and any change to the security envelope (Kind, AgentProfile, ApprovalDurationSeconds, Limits, Audit policy, HTTP/Git/SSH scopes — compared via reflect.DeepEqual) force-disables the grant and zeroes ApprovedAt/ExpiresAt. — src: internal/store/milestones.go:132-158, internal/store/capabilities.go:11-76,90-112 — pinned by: TestIssueCRUD, capabilities_test.go/TestCapabilityMaterialEditRevokesApproval, TestCapabilityAuditPolicyEditRevokesApproval — verify: automated test
- STORE-058: `DeleteCapability` removes the grant but deliberately retains its audit records (tombstone history); audits remain listable by capability id. — src: internal/store/capabilities.go:78-88 — pinned by: capabilities_test.go/TestCapabilityLifecycleAndAuditRetention — verify: automated test
- STORE-059: Capability audit pruning: `AddCapabilityAuditBounded` prunes in the same transaction — newest-first scan, drop when total>totalKeep (>0) or per-capability matches>perCapabilityKeep (>0); non-positive ceilings = unlimited. `PruneCapabilityAudits(id, keep)` clamps negative keep to 0. — src: internal/store/capabilities.go:114-196 — pinned by: capabilities_test.go/TestCapabilityLifecycleAndAuditRetention — verify: automated test

### Memory write-back (v5)

- STORE-060: `WriteMemory` validates before touching the DB: RequestID 1–128 chars, charset `[A-Za-z0-9._:-]`; Body non-empty valid UTF-8; WorkspaceGeneration≠0, SessionID 1–128 chars, AssociationRevision≠0; Kind ∈ {summary, decision, blocker, handoff}. Violations return errors wrapping `ErrInvalidMemoryWriteback` ("invalid memory write-back") with suffixes ": invalid request ID" / ": content is required" / ": source association is required" / `: unsupported kind %q`. — src: internal/store/memory_writeback.go:22-28,142-168 — pinned by: memory_writeback_test.go (indirect) — verify: automated test
- STORE-061: Idempotency: replay record key = RequestID; digest = SHA-256 of the JSON object `{"kind","body","target","target_id","plan_id","generation","session_id","revision"}` (Go encoding/json field order as declared). Same RequestID + same digest → replayed result (no mutation; returns stored note or the request body for summaries). Same RequestID + different digest → `ErrMemoryWritebackReplay` ("memory write-back request ID was already used"). — src: internal/store/memory_writeback.go:64-94,199-218 — pinned by: memory_writeback_test.go/TestWriteMemoryIdempotencyCollisionAndTargetValidationAreAtomic — verify: automated test
- STORE-062: Target revalidation inside the write tx: project target requires TargetID=PlanID=0; plan target requires PlanID==TargetID and the plan exists; task target requires task exists, task.PlanID==PlanID, and plan exists — else ErrInvalidMemoryWriteback with ": invalid project target" / ": plan target no longer exists" / ": invalid task target" / ": task target no longer exists" / ": task target changed plans" / `: unsupported target %q`. A failed validation writes nothing. — src: internal/store/memory_writeback.go:170-197 — pinned by: TestWriteMemoryIdempotencyCollisionAndTargetValidationAreAtomic, TestWriteMemorySerializesWithConcurrentTaskConversion — verify: automated test
- STORE-063: Summary writes replace Meta.Summary (meta.UpdatedAt/LastWriteVersion stamped, times in UTC here unlike other paths); decision/blocker/handoff writes create a Note with Kind set and CreatedAt UTC. Replay receipts are pruned to at most 256 records (MemoryWritebackReplayLimit), deleting the lowest Sequence, after each write. — src: internal/store/memory_writeback.go:16-20,99-140,220-245 — pinned by: TestWriteMemoryKindsScopesAndSummaryReplacement, TestWriteMemoryReplayReceiptsAreBounded — verify: automated test

### Bounded reads (DTO shapes consumed by GUI/agent callers)

- STORE-070: Bounded read limits must be 1..1000 inclusive, else error "bounded read limit must be between 1 and 1000". — src: internal/store/bounded.go:11,41-46 — pinned by: bounded_test.go/TestBoundedReadsRejectInvalidLimits — verify: automated test
- STORE-071: `Bounded[T]{Items, Total, More}` with JSON tags `items`, `total`, `more`; More = max(0, Total−len(Items)). Plans bounded list is oldest-first from cursor; RecentNotesBounded/RecentCommitsBounded/ListOpenIssues(Bounded) are newest-first (reverse cursor); Total is exact (bucket KeyN or full filtered scan). — src: internal/store/bounded.go:13-17,48-54,56-76,178-220,260-292 — pinned by: bounded_test.go/TestBoundedTrackingReadsReturnTotalsAndMore, TestBoundedRecentNotesCommitsAndIssuesAreNewestFirst — verify: automated test
- STORE-072: `ScanBounded[T]{Items, Scanned, ScanLimit, Truncated}` (no JSON tags) — hard scan of at most ScanLimit newest issues; Truncated=true iff older records exist beyond the scan window. — src: internal/store/bounded.go:19-27,226-258 — pinned by: bounded_test.go/TestOpenIssueScanBoundedReportsDeterministicHardLimit — verify: automated test
- STORE-073: `TaskProgress{Total, Done}` (JSON `total`, `done`) counts all tasks of a plan (Done = status done). `TaskAssociations{NoteCounts, CommitCounts, IssueCounts, LatestNotes}` maps keyed by task id; LatestNotes holds the newest note body per task; IssueCounts counts only open issues. Context-aware variants abort with ctx.Err(). — src: internal/store/bounded.go:29-39,110-142,294-355 — pinned by: bounded_test.go/TestPlanTaskProgressCountsBeyondReturnedTaskLimit, TestContextAwareBoundedScansHonorCancellation — verify: automated test
- STORE-074: `Counts{Milestones, MilestonesDone, Plans, PlansDone, Tasks, TasksDone, TasksBlocked, TasksOpen (status != done), Issues, IssuesOpen, Commits, Notes}` — Commits/Notes from bucket KeyN, the rest from decoded scans. No JSON tags (in-process DTO). — src: internal/store/store.go:609-677, internal/model/model.go:297-318 — pinned by: version_test.go/TestCounts, milestones_test.go/TestCountsIncludeMilestonesAndIssues — verify: automated test

### Backup & global registry

- STORE-080: `ptrack backup` copies the project DB file byte-for-byte (plain io.Copy of the live bbolt file — no bbolt online-backup API, no fsync, no lock coordination) to `$PTRACK_HOME/backups/<projectDirName>-<unixSeconds>.db` (dir created 0o755; name derives from the parent-of-.ptrack directory), then records it in the global `backups` bucket (best-effort: global-open/record errors are ignored) and prints the backup path + "\n" to stdout. — src: internal/cli/backup.go:16-48, internal/store/global.go:169-201 — pinned by: global_test.go/TestBackupProject — verify: automated test
- STORE-081: `RegisterProject` stores ProjectRef keyed by absolutized path (filepath.Abs, no symlink resolution), refreshing LastSeen on re-register. `ListProjects` returns all refs sorted LastSeen descending (stable). `ListRecentProjects(limit)`: limit≤0 → empty non-nil slice; limit>100 clamped to 100; otherwise newest-first top-N. — src: internal/store/global.go:90-159, internal/store/helpers.go:45-48 — pinned by: global_test.go/TestProjectRegistry, TestRecentProjectRegistryIsBounded — verify: automated test

### List ordering (externally visible via CLI/TUI/GUI output)

- STORE-090: ListPlans/ListTasks/ListMilestones: ascending Order (stable). ListNotes/ListCommits/ListIssues: ascending id (insertion order). RecentNotes(n): newest first; n≤0 or n>len → all. CommitsByTask/CommitsByPlan: newest first. ListCapabilityAudits: newest first; capabilityID 0 = all; limit≤0 = unlimited. ListCapabilities: creation order. — src: internal/store/store.go:222-240,304-333,552-607, internal/store/commits.go:48-86, internal/store/milestones.go:32-50,161-175, internal/store/capabilities.go:38-52,164-186 — pinned by: TestNotes, TestCommitAddDedupAndLink, TestCapabilityLifecycleAndAuditRetention — verify: automated test

## Notes / surprises

- **gob is the hard contract.** Every persisted struct is Go `encoding/gob` with
  self-describing type descriptors (Go type names like `model.Plan`,
  `store.memoryWritebackRecord`, field names, and gob's wire format including its
  zig-zag varints and per-field deltas). A Rust reimplementation must reproduce
  gob bytes exactly to share DB files with the Go build, or must own a
  format-6 migration. `time.Time` gob-encodes with local timezone offsets —
  most writes use local `time.Now()`, but WriteMemory paths use `.UTC()`, so
  mixed zone offsets exist on disk and survive round-trips.
- **bbolt file format + flock.** DB coexistence also pins the bbolt v1.5 page
  layout (meta pages, freelist) and the 1-second exclusive-lock timeout
  behavior. Crash recovery is entirely bbolt's (meta double-buffering); there is
  no app-level journal, and `ptrack backup` copies the live file with plain
  io.Copy — a backup taken concurrently with a writer can capture a torn page
  set (bbolt normally tolerates this via meta fallback, but nothing in ptrack
  coordinates it).
- **Sequence counters persist in bucket metadata.** Ids come from bbolt
  `NextSequence`, so deleting rows (task conversion, audit pruning, replay-receipt
  pruning) never reuses ids; `Order` fields are snapshots of `KeyN` at insert
  and can go stale relative to listing order after deletions — lists sort by
  Order, not id.
- **`backups` bucket values are tab-joined raw strings, not gob**, and keyed by
  UnixNano decimal strings — an inconsistent one-off encoding. Projects bucket is
  keyed by raw absolute path strings. Config is raw string/string.
- **Timezone/clock inconsistencies are observable**: meta mutations and CRUD use
  local time; memory write-back uses UTC; CAS compares `time.Time.Equal` (which
  is zone-insensitive for instants, but gob round-trips preserve zone offsets).
  Also `UpdateCapability` treats audit-policy edits as approval-revoking
  "material" changes (envelope includes the Audit field), which is
  security-relevant behavior a rewrite must keep.
