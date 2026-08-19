use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Barrier};
use std::time::Instant;

use ptrack_capability_policy::{AuditEvent, confirm_approval, normalize, sanitize_audit};
use ptrack_core::{
    Capability, CapabilityAudit, CapabilityAuditPolicy, CapabilityKind, CapabilityLimits, Digest32,
    GitScope, MIN_NATIVE_PAYLOAD_SCHEMA, MemoryKind, NativeRecord, NoteTarget, Plan, PlanStatus,
    RecordKind, TaskStatus, Timestamp, decode_record, encode_record_at_schema,
};

use crate::typed;
use crate::{
    ActiveBinding, ActorIdentity, Clock, Collection, GlobalStore, INVALID_CLAIM_PREFIX,
    MemoryWriteRequest, NATIVE_CODEC, NATIVE_PAYLOAD_SCHEMA, PlanDeleteSummary,
    ProjectRegistryCasResult, ProjectStore, RecordEnvelope, RecordKey, Store, StoreError,
    StoreKind,
};

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ptrack-typed-store-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        crate::protect_private_directory(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Clone, Copy)]
struct FixedClock(Timestamp);
impl Clock for FixedClock {
    fn now_local(&self) -> Timestamp {
        self.0
    }
    fn now_utc(&self) -> Timestamp {
        self.0
    }
}

#[test]
fn activated_reopens_share_one_process_writer_and_keep_raw_exclusion() {
    let temp = Temp::new();
    let path = temp.path("shared.redb");
    let expected = binding(&path, StoreKind::Project, "project-shared");
    let first =
        ProjectStore::create_new_with_clock(&path, expected.clone(), "first", clock()).unwrap();
    let second = ProjectStore::open_existing(&path, &expected, "second").unwrap();

    first.add_plan("shared", 0).unwrap();
    assert_eq!(second.snapshot().unwrap().plans[0].title, "shared");
    assert!(matches!(
        Store::open_existing(&path, StoreKind::Project),
        Err(StoreError::Busy)
    ));

    drop(second);
    drop(first);
    assert!(Store::open_existing(&path, StoreKind::Project).is_ok());
}

struct SteppingClock(AtomicI64);

impl SteppingClock {
    const fn new(seconds: i64) -> Self {
        Self(AtomicI64::new(seconds))
    }
}

impl Clock for SteppingClock {
    fn now_local(&self) -> Timestamp {
        timestamp(self.0.fetch_add(1, Ordering::Relaxed))
    }

    fn now_utc(&self) -> Timestamp {
        timestamp(self.0.fetch_add(1, Ordering::Relaxed))
    }
}

fn binding(path: &Path, kind: StoreKind, id: &str) -> ActiveBinding {
    ActiveBinding {
        generation: 7,
        database_id: id.to_owned(),
        kind,
        canonical_path: path
            .parent()
            .unwrap()
            .canonicalize()
            .unwrap()
            .join(path.file_name().unwrap()),
    }
}

fn clock() -> FixedClock {
    FixedClock(Timestamp::Fixed {
        seconds: 1_700_000_000,
        nanoseconds: 123,
        offset_seconds: 0,
    })
}

fn timestamp(seconds: i64) -> Timestamp {
    Timestamp::Fixed {
        seconds,
        nanoseconds: 0,
        offset_seconds: 0,
    }
}

#[test]
fn typed_project_mutations_conversion_cas_and_snapshot_are_atomic() {
    let temp = Temp::new();
    let path = temp.path("project.redb");
    let expected = binding(&path, StoreKind::Project, "project-1");
    let store =
        ProjectStore::create_new_with_clock(&path, expected.clone(), "test", clock()).unwrap();
    assert!(!store.application_writes().unwrap());

    let milestone = store.add_milestone("m").unwrap();
    assert!(store.application_writes().unwrap());
    let parent = store.add_plan("parent", milestone.id).unwrap();
    store.set_active_plan(parent.id).unwrap();
    let task = store.add_task(parent.id, "promote").unwrap();
    store
        .add_note(NoteTarget::Task, task.id, "decision")
        .unwrap();
    store
        .add_commit("abc", "subject", parent.id, task.id)
        .unwrap();
    store.add_issue("issue", "", None, task.id).unwrap();

    let error = store
        .compare_and_set_task_status(
            task.id,
            99,
            TaskStatus::Todo,
            task.updated_at,
            TaskStatus::Doing,
        )
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "task status changed: task #1 is plan #1/\"todo\" at 2023-11-14T22:13:20.000000123Z, expected plan #99/\"todo\" at 2023-11-14T22:13:20.000000123Z"
    );
    assert_eq!(store.task(task.id).unwrap().status, TaskStatus::Todo);
    let promoted = store.convert_task_to_plan(task.id).unwrap();
    assert_eq!(promoted.milestone_id, milestone.id);
    assert!(matches!(store.task(task.id), Err(StoreError::NotFound)));
    let snapshot = store.snapshot().unwrap();
    assert_eq!(snapshot.notes[0].target, NoteTarget::Plan);
    assert_eq!(snapshot.notes[0].target_id, promoted.id);
    assert_eq!(snapshot.commits[0].plan_id, promoted.id);
    assert_eq!(snapshot.commits[0].task_id, 0);
    assert_eq!(snapshot.issues[0].task_id, 0);

    drop(store);
    assert!(ProjectStore::open_existing(&path, &expected, "test").is_ok());
    let mut wrong = expected;
    wrong.generation += 1;
    assert!(matches!(
        ProjectStore::open_existing(&path, &wrong, "test"),
        Err(StoreError::ActivationBinding(_))
    ));
}

#[test]
fn convert_task_to_plan_carries_the_hold_reason_only_when_set() {
    let temp = Temp::new();
    let path = temp.path("convert-hold.redb");
    let expected = binding(&path, StoreKind::Project, "convert-hold");
    let store = ProjectStore::create_new_with_clock(&path, expected, "test", clock()).unwrap();

    let milestone = store.add_milestone("m").unwrap();
    let parent = store.add_plan("parent", milestone.id).unwrap();

    let held = store.add_task(parent.id, "held").unwrap();
    store.set_task_status(held.id, TaskStatus::Doing).unwrap();
    store
        .set_task_hold(held.id, Some("waiting on review".to_owned()))
        .unwrap();
    let promoted = store.convert_task_to_plan(held.id).unwrap();
    assert_eq!(promoted.status, PlanStatus::Active);
    assert_eq!(promoted.hold_reason.as_deref(), Some("waiting on review"));

    // A done task cannot be held today, so this pins the mapping rather than
    // the guard: `convert_task_to_plan` now filters the carried hold through
    // `plan_status_can_hold`, so a future status mapping that sends a held task
    // to a done or archived plan still cannot mint a done-and-held record.
    let done = store.add_task(parent.id, "done").unwrap();
    store.set_task_status(done.id, TaskStatus::Done).unwrap();
    let promoted_done = store.convert_task_to_plan(done.id).unwrap();
    assert_eq!(promoted_done.status, PlanStatus::Done);
    assert_eq!(promoted_done.hold_reason, None);
}

#[test]
fn first_plan_and_task_are_atomic_idempotent_and_fail_closed_on_ambiguity() {
    let temp = Temp::new();
    let path = temp.path("first-run.redb");
    let expected = binding(&path, StoreKind::Project, "first-run");
    let store =
        ProjectStore::create_new_with_clock(&path, expected, "test", SteppingClock::new(100))
            .unwrap();

    let injected = store
        .create_first_plan_inner("  First plan  ", || {
            Err(StoreError::InvalidFirstRun("injected".to_owned()))
        })
        .unwrap_err();
    assert_eq!(injected.to_string(), "invalid first-run mutation: injected");
    assert!(store.plans().unwrap().is_empty());
    assert_eq!(store.meta().unwrap().active_plan, 0);

    let plan = store.create_first_plan("  First plan  ").unwrap();
    assert_eq!(plan.id, 1);
    assert_eq!(plan.title, "First plan");
    assert_eq!(plan.status, PlanStatus::Active);
    assert_eq!(store.meta().unwrap().active_plan, plan.id);
    assert_eq!(store.create_first_plan("First plan").unwrap(), plan);
    assert!(store.create_first_plan("Different plan").is_err());
    assert_eq!(store.plans().unwrap(), std::slice::from_ref(&plan));

    let task = store.create_first_task(plan.id, "  First task  ").unwrap();
    assert_eq!(task.id, 1);
    assert_eq!(task.title, "First task");
    assert_eq!(task.status, TaskStatus::Todo);
    assert_eq!(
        store.create_first_task(plan.id, "First task").unwrap(),
        task
    );
    assert!(store.create_first_task(plan.id, "Different task").is_err());
    let doing = store.start_first_task(task.id, task.updated_at).unwrap();
    assert_eq!(doing.status, TaskStatus::Doing);
    assert_eq!(
        store.start_first_task(task.id, task.updated_at).unwrap(),
        doing
    );
    assert!(store.start_first_task(task.id, doing.updated_at).is_err());
    assert_eq!(
        store.create_first_task(plan.id, "First task").unwrap(),
        doing
    );

    let extra = store.add_task(plan.id, "manual concurrent task").unwrap();
    assert!(store.create_first_task(plan.id, "First task").is_err());
    assert!(store.start_first_task(task.id, doing.updated_at).is_err());
    assert_eq!(store.task(extra.id).unwrap().status, TaskStatus::Todo);
    store.add_plan("manual concurrent plan", 0).unwrap();
    assert!(store.create_first_plan("First plan").is_err());
}

#[test]
fn first_run_titles_are_trimmed_and_utf8_byte_bounded() {
    let temp = Temp::new();
    let path = temp.path("first-run-title.redb");
    let expected = binding(&path, StoreKind::Project, "first-run-title");
    let store = ProjectStore::create_new_with_clock(&path, expected, "test", clock()).unwrap();

    assert!(store.create_first_plan("   ").is_err());
    assert!(store.create_first_plan("é".repeat(121)).is_err());
    assert!(store.plans().unwrap().is_empty());
    let title = "é".repeat(120);
    assert_eq!(store.create_first_plan(&title).unwrap().title, title);
}

#[test]
fn concurrent_first_run_requests_never_create_duplicate_entities() {
    let temp = Temp::new();
    let same_path = temp.path("first-run-race-same.redb");
    let same = Arc::new(
        ProjectStore::create_new(
            &same_path,
            binding(&same_path, StoreKind::Project, "same"),
            "test",
        )
        .unwrap(),
    );
    let barrier = Arc::new(Barrier::new(3));
    let first = {
        let store = Arc::clone(&same);
        let barrier = Arc::clone(&barrier);
        std::thread::spawn(move || {
            barrier.wait();
            store.create_first_plan("same plan").map(|plan| plan.id)
        })
    };
    let second = {
        let store = Arc::clone(&same);
        let barrier = Arc::clone(&barrier);
        std::thread::spawn(move || {
            barrier.wait();
            store.create_first_plan(" same plan ").map(|plan| plan.id)
        })
    };
    barrier.wait();
    assert_eq!(
        first.join().unwrap().unwrap(),
        second.join().unwrap().unwrap()
    );
    assert_eq!(same.plans().unwrap().len(), 1);
    let barrier = Arc::new(Barrier::new(3));
    let first = {
        let store = Arc::clone(&same);
        let barrier = Arc::clone(&barrier);
        std::thread::spawn(move || {
            barrier.wait();
            store.create_first_task(1, "same task").map(|task| task.id)
        })
    };
    let second = {
        let store = Arc::clone(&same);
        let barrier = Arc::clone(&barrier);
        std::thread::spawn(move || {
            barrier.wait();
            store
                .create_first_task(1, " same task ")
                .map(|task| task.id)
        })
    };
    barrier.wait();
    assert_eq!(
        first.join().unwrap().unwrap(),
        second.join().unwrap().unwrap()
    );
    assert_eq!(same.tasks().unwrap().len(), 1);

    let different_path = temp.path("first-run-race-different.redb");
    let different = Arc::new(
        ProjectStore::create_new(
            &different_path,
            binding(&different_path, StoreKind::Project, "different"),
            "test",
        )
        .unwrap(),
    );
    let barrier = Arc::new(Barrier::new(3));
    let requests = ["alpha", "beta"].map(|title| {
        let store = Arc::clone(&different);
        let barrier = Arc::clone(&barrier);
        std::thread::spawn(move || {
            barrier.wait();
            store
                .create_first_plan(title)
                .map(|plan| plan.title)
                .map_err(|error| error.to_string())
        })
    });
    barrier.wait();
    let results = requests.map(|request| request.join().unwrap());
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| result
                .as_ref()
                .is_err_and(|error| error.contains("invalid first-run mutation")))
            .count(),
        1
    );
    assert_eq!(different.plans().unwrap().len(), 1);
    let barrier = Arc::new(Barrier::new(3));
    let requests = ["alpha task", "beta task"].map(|title| {
        let store = Arc::clone(&different);
        let barrier = Arc::clone(&barrier);
        std::thread::spawn(move || {
            barrier.wait();
            store
                .create_first_task(1, title)
                .map(|task| task.title)
                .map_err(|error| error.to_string())
        })
    });
    barrier.wait();
    let results = requests.map(|request| request.join().unwrap());
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(results.iter().filter(|result| result.is_err()).count(), 1);
    assert_eq!(different.tasks().unwrap().len(), 1);
}

#[test]
fn memory_writeback_is_idempotent_validated_and_bounded_reads_are_exact() {
    let temp = Temp::new();
    let path = temp.path("memory.redb");
    let store = ProjectStore::create_new_with_clock(
        &path,
        binding(&path, StoreKind::Project, "project-memory"),
        "test",
        clock(),
    )
    .unwrap();
    let plan = store.add_plan("plan", 0).unwrap();
    let task = store.add_task(plan.id, "task").unwrap();
    let request = MemoryWriteRequest {
        request_id: "request-1".to_owned(),
        kind: MemoryKind::Decision,
        body: "remember".to_owned(),
        target: NoteTarget::Task,
        target_id: task.id,
        plan_id: plan.id,
        workspace_generation: 7,
        session_id: "session".to_owned(),
        association_revision: 1,
    };
    assert!(!store.write_memory(request.clone()).unwrap().replayed);
    assert!(store.write_memory(request.clone()).unwrap().replayed);
    store
        .read(|transaction| {
            let envelope = transaction
                .get(Collection::MemoryWritebacks, RecordKey::Bytes(b"request-1"))?
                .expect("memory receipt");
            let NativeRecord::MemoryWriteback(receipt) =
                decode_record(RecordKind::MemoryWriteback, envelope.payload()).unwrap()
            else {
                panic!("wrong receipt kind");
            };
            assert_eq!(
                receipt.digest.0,
                [
                    0xc3, 0x59, 0x94, 0xfd, 0xc0, 0x8f, 0xb2, 0x3f, 0xdf, 0x10, 0x0a, 0xa5, 0x3a,
                    0x5c, 0x17, 0xe1, 0x7c, 0x7e, 0x68, 0x22, 0x73, 0x0e, 0x84, 0x0f, 0x48, 0x34,
                    0x16, 0x23, 0x5b, 0xf0, 0x2f, 0xac,
                ]
            );
            Ok(())
        })
        .unwrap();
    let before = store.snapshot().unwrap();
    let mut stale = request.clone();
    stale.request_id = "request-stale".to_owned();
    stale.workspace_generation = 6;
    assert_eq!(
        store.write_memory(stale).unwrap_err().to_string(),
        "stale workspace generation: expected 6, active 7"
    );
    assert_eq!(store.snapshot().unwrap(), before);
    let mut collision = request;
    collision.body = "different".to_owned();
    assert!(matches!(
        store.write_memory(collision),
        Err(StoreError::MemoryWritebackReplay)
    ));
    assert_eq!(store.recent_notes_bounded(1).unwrap().total, 1);
    assert!(matches!(
        store.plans_bounded(0),
        Err(StoreError::InvalidBoundedLimit)
    ));
    let expired = Instant::now();
    assert!(matches!(
        store.tasks_by_plan_bounded_until(1, 1, expired),
        Err(StoreError::DeadlineExceeded)
    ));
    assert!(matches!(
        store.task_associations_until(&BTreeSet::from([1]), expired),
        Err(StoreError::DeadlineExceeded)
    ));
    assert!(matches!(
        store.counts_until(expired),
        Err(StoreError::DeadlineExceeded)
    ));
}

#[test]
fn capability_audit_limits_match_unlimited_and_pruning_contracts() {
    let temp = Temp::new();
    let path = temp.path("audits.redb");
    let store = ProjectStore::create_new_with_clock(
        &path,
        binding(&path, StoreKind::Project, "project-audits"),
        "test",
        clock(),
    )
    .unwrap();
    let audit = |capability_id| CapabilityAudit {
        id: 0,
        capability_id,
        agent_profile: "agent".to_owned(),
        kind: CapabilityKind::Git,
        operation: "fetch".to_owned(),
        target: "origin".to_owned(),
        success: true,
        error_class: "none".to_owned(),
        duration_millis: 1,
        request_bytes: 0,
        response_bytes: 0,
        redirects: 0,
        created_at: Timestamp::Zero,
    };
    for capability_id in [1, 1, 1, 2] {
        store
            .add_capability_audit_bounded(audit(capability_id), 0, 0)
            .unwrap();
    }
    assert_eq!(store.capability_audits(0, 0).unwrap().len(), 4);
    store.add_capability_audit_bounded(audit(1), 2, 3).unwrap();
    assert_eq!(store.capability_audits(0, 0).unwrap().len(), 3);
    assert_eq!(store.capability_audits(1, 0).unwrap().len(), 2);
    store.prune_capability_audits(1, -1).unwrap();
    assert!(store.capability_audits(1, 0).unwrap().is_empty());
    assert_eq!(store.capability_audits(2, 0).unwrap().len(), 1);
}

#[test]
fn public_audit_api_prunes_to_fixed_global_ceiling() {
    let temp = Temp::new();
    let path = temp.path("public-audits.redb");
    let store = ProjectStore::create_new_with_clock(
        &path,
        binding(&path, StoreKind::Project, "project-public-audits"),
        "test",
        clock(),
    )
    .unwrap();
    let mut capability = Capability {
        id: 42,
        model_version: 1,
        revision: 1,
        name: "audit".to_owned(),
        kind: CapabilityKind::Git,
        agent_profile: "agent".to_owned(),
        enabled: false,
        approval_duration_seconds: 3_600,
        approved_at: Timestamp::Zero,
        expires_at: Timestamp::Zero,
        scope_digest: Digest32([1; 32]),
        limits: CapabilityLimits {
            timeout_seconds: 30,
            max_request_bytes: 1_024,
            max_response_bytes: 1_024,
            max_output_bytes: 1_024,
            max_redirects: 0,
            max_concurrent: 1,
        },
        audit: CapabilityAuditPolicy {
            enabled: true,
            retain_last: 1_000,
        },
        http: None,
        git: None,
        ssh: None,
        created_at: Timestamp::Zero,
        updated_at: Timestamp::Zero,
    };
    let event = AuditEvent {
        operation: "fetch".to_owned(),
        target: "origin".to_owned(),
        success: true,
        error_class: String::new(),
        duration_millis: 0,
        request_bytes: 0,
        response_bytes: 0,
        redirects: 0,
    };
    store.seed_capability_audits(5_000).unwrap();
    capability.id = 10_000;
    let appended = store
        .record_capability_audit(sanitize_audit(&capability, &event).unwrap())
        .unwrap();
    assert_eq!(appended.id, 5_001);
    let audits = store.capability_audits(0, 0).unwrap();
    assert_eq!(audits.len(), 5_000);
    assert_eq!(audits.first().unwrap().id, 5_001);
    assert_eq!(audits.last().unwrap().id, 2);
    assert!(!audits.iter().any(|audit| audit.id == 1));
}

#[test]
fn raw_redb_secret_canary_is_rejected_on_reopen() {
    let temp = Temp::new();
    let path = temp.path("raw-secret.redb");
    let expected = binding(&path, StoreKind::Project, "project-raw-secret");
    let store =
        ProjectStore::create_new_with_clock(&path, expected.clone(), "test", clock()).unwrap();
    let mut capability = Capability {
        id: 1,
        model_version: 1,
        revision: 1,
        name: "audit".to_owned(),
        kind: CapabilityKind::Git,
        agent_profile: "agent".to_owned(),
        enabled: false,
        approval_duration_seconds: 3_600,
        approved_at: Timestamp::Zero,
        expires_at: Timestamp::Zero,
        scope_digest: Digest32([1; 32]),
        limits: CapabilityLimits {
            timeout_seconds: 30,
            max_request_bytes: 1_024,
            max_response_bytes: 1_024,
            max_output_bytes: 1_024,
            max_redirects: 0,
            max_concurrent: 1,
        },
        audit: CapabilityAuditPolicy {
            enabled: true,
            retain_last: 1,
        },
        http: None,
        git: None,
        ssh: None,
        created_at: Timestamp::Zero,
        updated_at: Timestamp::Zero,
    };
    let event = AuditEvent {
        operation: "fetch".to_owned(),
        target: "origin".to_owned(),
        success: true,
        error_class: String::new(),
        duration_millis: 0,
        request_bytes: 0,
        response_bytes: 0,
        redirects: 0,
    };
    store
        .record_capability_audit(sanitize_audit(&capability, &event).unwrap())
        .unwrap();
    drop(store);
    crate::project_test_support::inject_raw_audit_secret(&path);
    assert!(matches!(
        ProjectStore::open_existing(&path, &expected, "test"),
        Err(StoreError::InvalidManifest(_))
    ));
    capability.audit.enabled = false;
    assert!(sanitize_audit(&capability, &event).is_none());
}

#[test]
fn public_audit_path_never_persists_secret_bearing_event_fields() {
    const SECRET: &str = "super-secret-audit-canary-7c94";
    let temp = Temp::new();
    let path = temp.path("sanitized-audit.redb");
    let expected = binding(&path, StoreKind::Project, "project-sanitized-audit");
    let store =
        ProjectStore::create_new_with_clock(&path, expected.clone(), "test", clock()).unwrap();
    let capability = Capability {
        id: 7,
        model_version: 1,
        revision: 1,
        name: "audit".to_owned(),
        kind: CapabilityKind::Http,
        agent_profile: "agent".to_owned(),
        enabled: false,
        approval_duration_seconds: 3_600,
        approved_at: Timestamp::Zero,
        expires_at: Timestamp::Zero,
        scope_digest: Digest32([1; 32]),
        limits: CapabilityLimits {
            timeout_seconds: 30,
            max_request_bytes: 1_024,
            max_response_bytes: 1_024,
            max_output_bytes: 1_024,
            max_redirects: 0,
            max_concurrent: 1,
        },
        audit: CapabilityAuditPolicy {
            enabled: true,
            retain_last: 10,
        },
        http: None,
        git: None,
        ssh: None,
        created_at: Timestamp::Zero,
        updated_at: Timestamp::Zero,
    };
    let event = AuditEvent {
        operation: "GET".to_owned(),
        target: format!("https://example.com/private?token={SECRET}"),
        success: false,
        error_class: format!("raw stderr {SECRET}"),
        duration_millis: 1,
        request_bytes: 2,
        response_bytes: 3,
        redirects: 0,
    };
    let persisted = store
        .record_capability_audit(sanitize_audit(&capability, &event).unwrap())
        .unwrap();
    assert_eq!(persisted.target, "https://example.com");
    assert_eq!(persisted.error_class, "internal");
    let decoded = store.capability_audits(capability.id, 1).unwrap();
    assert_eq!(decoded, [persisted]);
    drop(store);
    let raw = fs::read(&path).unwrap();
    assert!(
        !raw.windows(SECRET.len())
            .any(|window| window == SECRET.as_bytes())
    );
    let reopened = ProjectStore::open_existing(&path, &expected, "test").unwrap();
    assert_eq!(
        reopened.capability_audits(capability.id, 1).unwrap(),
        decoded
    );
}

#[test]
fn capability_crud_cannot_mint_or_edit_approval_state() {
    let temp = Temp::new();
    let path = temp.path("capabilities.redb");
    let store = ProjectStore::create_new_with_clock(
        &path,
        binding(&path, StoreKind::Project, "project-capabilities"),
        "test",
        clock(),
    )
    .unwrap();
    let approval = clock().0;
    let mut value = Capability {
        id: 99,
        model_version: 1,
        revision: 99,
        name: "git".to_owned(),
        kind: CapabilityKind::Git,
        agent_profile: "agent".to_owned(),
        enabled: true,
        approval_duration_seconds: 3600,
        approved_at: approval,
        expires_at: Timestamp::Fixed {
            seconds: 1_700_003_600,
            nanoseconds: 123,
            offset_seconds: 0,
        },
        scope_digest: Digest32([1; 32]),
        limits: CapabilityLimits {
            timeout_seconds: 30,
            max_request_bytes: 1024,
            max_response_bytes: 1024,
            max_output_bytes: 1024,
            max_redirects: 0,
            max_concurrent: 1,
        },
        audit: CapabilityAuditPolicy {
            enabled: true,
            retain_last: 10,
        },
        http: None,
        git: Some(GitScope {
            remote_name: "origin".to_owned(),
            remote_url: "https://example.test/repo.git".to_owned(),
            operations: vec!["fetch".to_owned()],
            branches: vec!["main".to_owned()],
            refspecs: Vec::new(),
            allow_tags: false,
            allow_force_push: false,
            allow_delete_refs: false,
        }),
        ssh: None,
        created_at: Timestamp::Zero,
        updated_at: Timestamp::Zero,
    };
    value = store.add_capability(value).unwrap();
    assert!(!value.enabled);
    assert!(value.approved_at.is_zero());
    assert!(value.expires_at.is_zero());

    let illicit_approval = value.expires_at;
    value.enabled = true;
    value.approved_at = approval;
    value.expires_at = illicit_approval;
    store.update_capability(value.clone()).unwrap();
    let persisted = store.capability(value.id).unwrap();
    assert!(!persisted.enabled);
    assert!(persisted.approved_at.is_zero());
    assert!(persisted.expires_at.is_zero());
}

#[test]
fn capability_revision_and_lifecycle_cas_are_fail_closed() {
    let temp = Temp::new();
    let path = temp.path("capability-cas.redb");
    let store = ProjectStore::create_new_with_clock(
        &path,
        binding(&path, StoreKind::Project, "project-capability-cas"),
        "test",
        clock(),
    )
    .unwrap();
    let mut capability = Capability {
        id: 0,
        model_version: 1,
        revision: 0,
        name: "git".to_owned(),
        kind: CapabilityKind::Git,
        agent_profile: "agent".to_owned(),
        enabled: false,
        approval_duration_seconds: 3_600,
        approved_at: Timestamp::Zero,
        expires_at: Timestamp::Zero,
        scope_digest: Digest32::EMPTY,
        limits: CapabilityLimits {
            timeout_seconds: 30,
            max_request_bytes: 1_024,
            max_response_bytes: 1_024,
            max_output_bytes: 1_024,
            max_redirects: 0,
            max_concurrent: 1,
        },
        audit: CapabilityAuditPolicy {
            enabled: true,
            retain_last: 10,
        },
        http: None,
        git: Some(GitScope {
            remote_name: "origin".to_owned(),
            remote_url: "https://example.test/repo.git".to_owned(),
            operations: vec!["fetch".to_owned()],
            branches: vec!["main".to_owned()],
            refspecs: Vec::new(),
            allow_tags: false,
            allow_force_push: false,
            allow_delete_refs: false,
        }),
        ssh: None,
        created_at: Timestamp::Zero,
        updated_at: Timestamp::Zero,
    };
    capability = normalize(&capability).unwrap().capability;
    capability = store.add_capability(capability).unwrap();
    let stale = capability.clone();
    capability.name = "renamed".to_owned();
    capability = store.update_capability(capability).unwrap();
    assert!(matches!(
        store.update_capability(stale),
        Err(StoreError::CapabilityRevisionChanged { .. })
    ));
    assert!(confirm_approval(&capability, Digest32([8; 32])).is_err());
    let proof = confirm_approval(&capability, capability.scope_digest).unwrap();
    capability = store.approve_capability(proof).unwrap();
    assert_eq!(
        capability.expires_at.unix_nanoseconds(),
        clock()
            .0
            .unix_nanoseconds()
            .map(|value| value + 3_600_000_000_000)
    );
    assert!(matches!(
        store.delete_capability(capability.id, capability.revision - 1),
        Err(StoreError::CapabilityRevisionChanged { .. })
    ));
    store
        .delete_capability(capability.id, capability.revision)
        .unwrap();
    assert!(matches!(
        store.capability(capability.id),
        Err(StoreError::NotFound)
    ));
}

#[test]
fn executable_capability_store_contract_coverage() {
    let checks: [fn(); 1] = [assert_cap_030_approved_security_edit_revokes];
    for check in checks {
        check();
    }
}

fn assert_cap_030_approved_security_edit_revokes() {
    const START: i64 = 1_800_000_000;
    let temp = Temp::new();
    let path = temp.path("capability-security-edit.redb");
    let store = ProjectStore::create_new_with_clock(
        &path,
        binding(
            &path,
            StoreKind::Project,
            "project-capability-security-edit",
        ),
        "test",
        SteppingClock::new(START),
    )
    .unwrap();
    let mut draft = Capability {
        id: 0,
        model_version: 0,
        revision: 0,
        name: "repository".to_owned(),
        kind: CapabilityKind::Git,
        agent_profile: "agent".to_owned(),
        enabled: false,
        approval_duration_seconds: 0,
        approved_at: Timestamp::Zero,
        expires_at: Timestamp::Zero,
        scope_digest: Digest32::EMPTY,
        limits: CapabilityLimits {
            timeout_seconds: 0,
            max_request_bytes: 0,
            max_response_bytes: 0,
            max_output_bytes: 0,
            max_redirects: 0,
            max_concurrent: 0,
        },
        audit: CapabilityAuditPolicy {
            enabled: false,
            retain_last: 0,
        },
        http: None,
        git: Some(GitScope {
            remote_name: "origin".to_owned(),
            remote_url: "https://example.com/repo.git".to_owned(),
            operations: vec!["fetch".to_owned()],
            branches: vec!["main".to_owned()],
            refspecs: Vec::new(),
            allow_tags: false,
            allow_force_push: false,
            allow_delete_refs: false,
        }),
        ssh: None,
        created_at: Timestamp::Zero,
        updated_at: Timestamp::Zero,
    };
    draft = normalize(&draft).unwrap().capability;
    let stored = store.add_capability(draft).unwrap();
    assert_eq!(stored.created_at, timestamp(START + 1));
    assert_eq!(stored.updated_at, timestamp(START + 1));

    let initial_proof = confirm_approval(&stored, stored.scope_digest).unwrap();
    let approved = store.approve_capability(initial_proof).unwrap();
    assert!(approved.enabled);
    assert_eq!(approved.revision, stored.revision + 1);
    assert_eq!(approved.approved_at, timestamp(START + 2));
    assert_eq!(approved.updated_at, timestamp(START + 2));
    let stale_proof = confirm_approval(&approved, approved.scope_digest).unwrap();

    let mut edit = approved.clone();
    edit.git
        .as_mut()
        .unwrap()
        .operations
        .push("push".to_owned());
    edit = normalize(&edit).unwrap().capability;
    assert_ne!(edit.scope_digest, approved.scope_digest);
    let updated = store.update_capability(edit).unwrap();
    assert_eq!(updated.id, approved.id);
    assert_eq!(updated.created_at, approved.created_at);
    assert_eq!(updated.revision, approved.revision + 1);
    assert_eq!(updated.updated_at, timestamp(START + 3));
    assert!(!updated.enabled);
    assert!(updated.approved_at.is_zero());
    assert!(updated.expires_at.is_zero());
    assert_ne!(updated.scope_digest, approved.scope_digest);

    assert!(matches!(
        store.approve_capability(stale_proof),
        Err(StoreError::CapabilityRevisionChanged {
            expected,
            actual,
        }) if expected == approved.revision && actual == updated.revision
    ));
    let refreshed_proof = confirm_approval(&updated, updated.scope_digest).unwrap();
    let reapproved = store.approve_capability(refreshed_proof).unwrap();
    assert!(reapproved.enabled);
    assert_eq!(reapproved.revision, updated.revision + 1);
    assert_eq!(reapproved.approved_at, timestamp(START + 5));
}

#[test]
fn global_bytes_and_transaction_consistent_backup_are_create_only() {
    let temp = Temp::new();
    let global_path = temp.path("settings.redb");
    let global = GlobalStore::create_new_with_clock(
        &global_path,
        binding(&global_path, StoreKind::Global, "global"),
        clock(),
    )
    .unwrap();
    global.set_config(b"binary", b"\0\xff").unwrap();
    assert_eq!(global.config(b"binary").unwrap(), b"\0\xff");

    let project_path = temp.path("backup-source.redb");
    let project = ProjectStore::create_new_with_clock(
        &project_path,
        binding(&project_path, StoreKind::Project, "backup-source"),
        "test",
        clock(),
    )
    .unwrap();
    project.add_plan("durable", 0).unwrap();
    let backup = temp.path("backups/copy.redb");
    project.backup_to(&backup).unwrap();
    let before = fs::read(&backup).unwrap();
    assert!(!before.is_empty());
    assert!(matches!(
        project.backup_to(&backup),
        Err(StoreError::DestinationExists { .. })
    ));
    assert_eq!(fs::read(backup).unwrap(), before);

    let corrupt = temp.path("backups/corrupt.redb");
    let error = project
        .backup_to_with_after_copy(&corrupt, |path| {
            fs::write(path, b"not a database")?;
            Ok(())
        })
        .unwrap_err();
    assert!(matches!(
        error,
        StoreError::Engine(_) | StoreError::InvalidManifest(_)
    ));
    assert!(!corrupt.exists());
}

#[test]
fn global_config_updates_read_and_write_in_one_transaction() {
    let temp = Temp::new();
    let path = temp.path("update-config.redb");
    let global = GlobalStore::create_new_with_clock(
        &path,
        binding(&path, StoreKind::Global, "update-config"),
        clock(),
    )
    .unwrap();

    // An absent record reads as empty bytes, and the update sees every write
    // already committed for that key.
    let seen = global
        .update_config(b"counter", |stored| Ok((b"one".to_vec(), stored.to_vec())))
        .unwrap();
    assert!(seen.is_empty());
    let seen = global
        .update_config(b"counter", |stored| Ok((b"two".to_vec(), stored.to_vec())))
        .unwrap();
    assert_eq!(seen, b"one");
    assert_eq!(global.config(b"counter").unwrap(), b"two");

    // A failing update propagates and never degrades the stored record.
    assert!(matches!(
        global.update_config::<()>(b"counter", |_| Err(StoreError::NotFound)),
        Err(StoreError::NotFound)
    ));
    assert_eq!(global.config(b"counter").unwrap(), b"two");
    assert!(
        global
            .update_config(b"", |stored| Ok((stored.to_vec(), ())))
            .is_err()
    );
}

#[test]
fn project_registry_normalizes_lexical_aliases() {
    let temp = Temp::new();
    let global_path = temp.path("registry.redb");
    let global = GlobalStore::create_new_with_clock(
        &global_path,
        binding(&global_path, StoreKind::Global, "registry"),
        clock(),
    )
    .unwrap();
    let root = temp.path("project");
    fs::create_dir(&root).unwrap();
    let alias = root.join("child/../.");
    let first = global.register_project("one", &alias).unwrap();
    let second = global.register_project("two", &root).unwrap();
    assert_eq!(first.path, second.path);
    let projects = global.projects().unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].name, "two");
}

#[test]
fn project_registry_forget_and_relocation_are_compare_and_swap_idempotent() {
    let temp = Temp::new();
    let global_path = temp.path("registry-cas.redb");
    let global = GlobalStore::create_new_with_clock(
        &global_path,
        binding(&global_path, StoreKind::Global, "registry-cas"),
        SteppingClock::new(100),
    )
    .unwrap();
    let first_root = temp.path("first");
    let second_root = temp.path("second");
    fs::create_dir(&first_root).unwrap();
    fs::create_dir(&second_root).unwrap();
    let sentinel = first_root.join("sentinel");
    fs::write(&sentinel, b"keep").unwrap();

    let first = global.register_project("first", &first_root).unwrap();
    let replacement = global.register_project("renamed", &first_root).unwrap();
    assert_eq!(
        global.forget_project_if_matches(&first).unwrap(),
        ProjectRegistryCasResult::Stale
    );
    let relocated = global
        .relocate_project_if_matches(&replacement, "second", &second_root)
        .unwrap();
    let ProjectRegistryCasResult::Applied(relocated) = relocated else {
        panic!("expected relocation");
    };
    assert_eq!(global.projects().unwrap(), vec![relocated.clone()]);
    assert!(sentinel.exists());
    assert_eq!(
        global.forget_project_if_matches(&replacement).unwrap(),
        ProjectRegistryCasResult::Absent
    );
    assert!(matches!(
        global.forget_project_if_matches(&relocated).unwrap(),
        ProjectRegistryCasResult::Applied(_)
    ));
    assert_eq!(
        global.forget_project_if_matches(&relocated).unwrap(),
        ProjectRegistryCasResult::Absent
    );
    assert!(sentinel.exists());
}

#[test]
fn project_registry_touch_strictly_advances_an_equal_clock() {
    let temp = Temp::new();
    let global_path = temp.path("registry-touch.redb");
    let global = GlobalStore::create_new_with_clock(
        &global_path,
        binding(&global_path, StoreKind::Global, "registry-touch"),
        FixedClock(timestamp(100)),
    )
    .unwrap();
    let root = temp.path("touch");
    fs::create_dir(&root).unwrap();
    let first = global.register_project("touch", &root).unwrap();
    let ProjectRegistryCasResult::Applied(touched) = global
        .relocate_project_if_matches(&first, "touch", &root)
        .unwrap()
    else {
        panic!("expected touch");
    };
    assert!(
        touched.last_seen.unix_nanoseconds().unwrap() > first.last_seen.unix_nanoseconds().unwrap()
    );
}

#[test]
fn project_registry_touch_rejects_an_exhausted_timestamp() {
    let temp = Temp::new();
    let global_path = temp.path("registry-touch-exhausted.redb");
    let exhausted = Timestamp::Fixed {
        seconds: i64::MAX,
        nanoseconds: 999_999_999,
        offset_seconds: 0,
    };
    let global = GlobalStore::create_new_with_clock(
        &global_path,
        binding(&global_path, StoreKind::Global, "registry-touch-exhausted"),
        FixedClock(exhausted),
    )
    .unwrap();
    let root = temp.path("touch-exhausted");
    fs::create_dir(&root).unwrap();
    let first = global.register_project("touch", &root).unwrap();
    assert!(matches!(
        global.relocate_project_if_matches(&first, "touch", &root),
        Err(StoreError::InvalidManifest(message)) if message == "project registry timestamp is exhausted"
    ));
    assert_eq!(global.projects().unwrap(), vec![first]);
}

#[cfg(unix)]
#[test]
fn typed_write_rejects_path_replacement_before_mutation() {
    let temp = Temp::new();
    let path = temp.path("identity.redb");
    let store = ProjectStore::create_new_with_clock(
        &path,
        binding(&path, StoreKind::Project, "identity"),
        "test",
        clock(),
    )
    .unwrap();
    let moved = temp.path("moved.redb");
    fs::rename(&path, &moved).unwrap();
    fs::write(&path, b"replacement").unwrap();
    assert!(matches!(
        store.set_goal("must not commit"),
        Err(StoreError::PathChanged { .. })
    ));
    assert_eq!(store.meta().unwrap().goal, "");
    drop(store);
    fs::remove_file(path).unwrap();
    fs::rename(moved, temp.path("identity.redb")).unwrap();
}

#[test]
fn holds_round_trip_and_are_refused_on_terminal_records() {
    let temp = Temp::new();
    let path = temp.path("hold.redb");
    let expected = binding(&path, StoreKind::Project, "project-hold");
    let store = ProjectStore::create_new_with_clock(&path, expected, "test", clock()).unwrap();

    let plan = store.add_plan("plan", 0).unwrap();
    let task = store.add_task(plan.id, "task").unwrap();
    assert_eq!(plan.hold_reason, None);
    assert_eq!(task.hold_reason, None);

    store
        .set_plan_hold(plan.id, Some("waiting on review".to_owned()))
        .unwrap();
    store
        .set_task_hold(task.id, Some("blocked upstream".to_owned()))
        .unwrap();
    assert_eq!(
        store.plan(plan.id).unwrap().hold_reason.as_deref(),
        Some("waiting on review")
    );
    assert_eq!(
        store.task(task.id).unwrap().hold_reason.as_deref(),
        Some("blocked upstream")
    );

    // Surrounding whitespace is trimmed at the store, the one path every writer
    // shares, so the CLI and the app mutation store the same words the same way.
    store
        .set_plan_hold(plan.id, Some("  waiting on review  ".to_owned()))
        .unwrap();
    store
        .set_task_hold(task.id, Some("\tblocked upstream\n".to_owned()))
        .unwrap();
    assert_eq!(
        store.plan(plan.id).unwrap().hold_reason.as_deref(),
        Some("waiting on review")
    );
    assert_eq!(
        store.task(task.id).unwrap().hold_reason.as_deref(),
        Some("blocked upstream")
    );

    // A held record survives a reopen, so the hold is durable and not derived.
    drop(store);
    let expected = binding(&path, StoreKind::Project, "project-hold");
    let store = ProjectStore::open_existing(&path, &expected, "test").unwrap();
    assert_eq!(
        store.plan(plan.id).unwrap().hold_reason.as_deref(),
        Some("waiting on review")
    );

    // Resuming is always allowed, including from a terminal state.
    store.set_task_hold(task.id, None).unwrap();
    assert_eq!(store.task(task.id).unwrap().hold_reason, None);
    store.set_task_status(task.id, TaskStatus::Done).unwrap();
    store.set_task_hold(task.id, None).unwrap();

    let error = store
        .set_task_hold(task.id, Some("too late".to_owned()))
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        format!(
            "invalid hold mutation: task #{} is done and cannot be put on hold",
            task.id
        )
    );

    store.set_plan_hold(plan.id, None).unwrap();
    for status in [PlanStatus::Done, PlanStatus::Archived] {
        store.set_plan_status(plan.id, status).unwrap();
        store.set_plan_hold(plan.id, None).unwrap();
        assert!(matches!(
            store.set_plan_hold(plan.id, Some("too late".to_owned())),
            Err(StoreError::InvalidHold(_))
        ));
    }

    assert!(matches!(
        store.set_plan_hold(9_999, None),
        Err(StoreError::NotFound)
    ));

    // Reason bounds are enforced at the core trust boundary, so a malformed
    // reason never reaches the database.
    store.set_plan_status(plan.id, PlanStatus::Active).unwrap();
    for bad in [
        "   ".to_owned(),
        "line\nbreak".to_owned(),
        "x".repeat(1_025),
    ] {
        assert!(store.set_plan_hold(plan.id, Some(bad)).is_err());
    }
    assert_eq!(store.plan(plan.id).unwrap().hold_reason, None);
}

/// A status transition into a terminal state must clear the hold, because
/// `set_*_hold` refuses to create the done-and-held record that would otherwise
/// persist.
#[test]
fn terminal_status_transitions_clear_the_hold_reason() {
    let temp = Temp::new();
    let path = temp.path("hold-clear.redb");
    let expected = binding(&path, StoreKind::Project, "project-hold-clear");
    let store = ProjectStore::create_new_with_clock(&path, expected, "test", clock()).unwrap();
    let plan = store.add_plan("plan", 0).unwrap();
    let task = store.add_task(plan.id, "task").unwrap();

    // A task keeps its hold while it stays open, in either direction.
    store
        .set_task_hold(task.id, Some("blocked upstream".to_owned()))
        .unwrap();
    for status in [TaskStatus::Doing, TaskStatus::Blocked, TaskStatus::Todo] {
        store.set_task_status(task.id, status).unwrap();
        assert_eq!(
            store.task(task.id).unwrap().hold_reason.as_deref(),
            Some("blocked upstream"),
            "{status:?}"
        );
    }
    store.set_task_status(task.id, TaskStatus::Done).unwrap();
    assert_eq!(store.task(task.id).unwrap().hold_reason, None);

    // The compare-and-set path has the same hole and the same fix.
    store.set_task_status(task.id, TaskStatus::Todo).unwrap();
    store
        .set_task_hold(task.id, Some("blocked upstream".to_owned()))
        .unwrap();
    let held = store.task(task.id).unwrap();
    let updated = store
        .compare_and_set_task_status(
            task.id,
            plan.id,
            TaskStatus::Todo,
            held.updated_at,
            TaskStatus::Done,
        )
        .unwrap();
    assert_eq!(updated.hold_reason, None);
    assert_eq!(store.task(task.id).unwrap().hold_reason, None);

    for status in [PlanStatus::Done, PlanStatus::Archived] {
        store.set_plan_status(plan.id, PlanStatus::Active).unwrap();
        store
            .set_plan_hold(plan.id, Some("waiting on review".to_owned()))
            .unwrap();
        store.set_plan_status(plan.id, status).unwrap();
        assert_eq!(store.plan(plan.id).unwrap().hold_reason, None, "{status:?}");
    }
}

/// The regression the reviewer caught: opening an existing database
/// re-validates every stored record, so a build that pinned the current payload
/// schema there refused every database written before the bump.
#[test]
fn a_database_of_schema_1_records_opens_reads_and_upgrades_on_write() {
    let temp = Temp::new();
    let path = temp.path("schema-1-database.redb");
    let expected = binding(&path, StoreKind::Project, "project-schema-1");
    let store = ProjectStore::create_new_with_clock(&path, expected, "test", clock()).unwrap();
    let plan = store.add_plan("legacy plan", 0).unwrap();
    let task = store.add_task(plan.id, "legacy task").unwrap();

    // Rewrite both records exactly as a released pre-hold build stored them:
    // the schema-1 layout, under payload schema 1.
    store
        .write(|transaction| {
            let plan_payload = encode_record_at_schema(
                &NativeRecord::Plan(plan.clone()),
                MIN_NATIVE_PAYLOAD_SCHEMA,
            )
            .unwrap();
            let task_payload = encode_record_at_schema(
                &NativeRecord::Task(task.clone()),
                MIN_NATIVE_PAYLOAD_SCHEMA,
            )
            .unwrap();
            for (collection, id, payload) in [
                (Collection::Plans, plan.id, plan_payload),
                (Collection::Tasks, task.id, task_payload),
            ] {
                let legacy = RecordEnvelope::new(NATIVE_CODEC, MIN_NATIVE_PAYLOAD_SCHEMA, payload);
                transaction.put(collection, RecordKey::Id(id), &legacy)?;
            }
            Ok(())
        })
        .unwrap();
    drop(store);

    let expected = binding(&path, StoreKind::Project, "project-schema-1");
    let store = ProjectStore::open_existing(&path, &expected, "test").unwrap();
    assert_eq!(store.plan(plan.id).unwrap(), plan);
    assert_eq!(store.task(task.id).unwrap(), task);
    assert_eq!(stored_schema(&store, Collection::Plans, plan.id), 1);
    assert_eq!(stored_schema(&store, Collection::Tasks, task.id), 1);

    // Any write upgrades that one record in place; nothing else is rewritten.
    store
        .set_plan_hold(plan.id, Some("waiting on review".to_owned()))
        .unwrap();
    assert_eq!(
        stored_schema(&store, Collection::Plans, plan.id),
        NATIVE_PAYLOAD_SCHEMA
    );
    assert_eq!(stored_schema(&store, Collection::Tasks, task.id), 1);

    // The half-upgraded database still opens, and the untouched schema-1 task
    // still reads.
    drop(store);
    let expected = binding(&path, StoreKind::Project, "project-schema-1");
    let store = ProjectStore::open_existing(&path, &expected, "test").unwrap();
    assert_eq!(
        store.plan(plan.id).unwrap().hold_reason.as_deref(),
        Some("waiting on review")
    );
    assert_eq!(store.task(task.id).unwrap(), task);
}

/// The 0.26-era analogue of the schema-1 test above: a database whose records
/// were written at payload schema 2 (hold reasons present, no actor/claim
/// fields) opens as-is, reads correctly, and upgrades lazily per record on
/// write. This is the exact upgrade path every existing 0.26 database takes.
#[test]
fn a_database_of_schema_2_records_opens_reads_and_upgrades_on_write() {
    let temp = Temp::new();
    let path = temp.path("schema-2-database.redb");
    let expected = binding(&path, StoreKind::Project, "project-schema-2");
    let store = ProjectStore::create_new_with_clock(&path, expected, "test", clock()).unwrap();
    let plan = store.add_plan("held plan", 0).unwrap();
    store
        .set_plan_hold(plan.id, Some("waiting on review".to_owned()))
        .unwrap();
    let plan = store.plan(plan.id).unwrap();
    let task = store.add_task(plan.id, "schema-2 task").unwrap();
    let meta = store.meta().unwrap();

    // Rewrite all three records exactly as a released 0.26 build stored them:
    // payload schema 2, hold reason present, none of the schema-3 fields.
    store
        .write(|transaction| {
            let plan_payload =
                encode_record_at_schema(&NativeRecord::Plan(plan.clone()), 2).unwrap();
            let task_payload =
                encode_record_at_schema(&NativeRecord::Task(task.clone()), 2).unwrap();
            let meta_payload =
                encode_record_at_schema(&NativeRecord::Meta(meta.clone()), 2).unwrap();
            transaction.put(
                Collection::Plans,
                RecordKey::Id(plan.id),
                &RecordEnvelope::new(NATIVE_CODEC, 2, plan_payload),
            )?;
            transaction.put(
                Collection::Tasks,
                RecordKey::Id(task.id),
                &RecordEnvelope::new(NATIVE_CODEC, 2, task_payload),
            )?;
            transaction.put(
                Collection::ProjectMeta,
                RecordKey::Singleton,
                &RecordEnvelope::new(NATIVE_CODEC, 2, meta_payload),
            )?;
            Ok(())
        })
        .unwrap();
    drop(store);

    // The database opens without any migration step and reads exactly.
    let expected = binding(&path, StoreKind::Project, "project-schema-2");
    let store = ProjectStore::open_existing(&path, &expected, "test").unwrap();
    assert_eq!(store.plan(plan.id).unwrap(), plan);
    assert_eq!(store.task(task.id).unwrap(), task);
    assert_eq!(stored_schema(&store, Collection::Plans, plan.id), 2);
    assert_eq!(stored_schema(&store, Collection::Tasks, task.id), 2);

    // One write upgrades that one record to schema 3; nothing else moves.
    store.set_task_title(task.id, "renamed").unwrap();
    assert_eq!(
        stored_schema(&store, Collection::Tasks, task.id),
        NATIVE_PAYLOAD_SCHEMA
    );
    assert_eq!(stored_schema(&store, Collection::Plans, plan.id), 2);

    // The half-upgraded database still opens and both records still read.
    drop(store);
    let expected = binding(&path, StoreKind::Project, "project-schema-2");
    let store = ProjectStore::open_existing(&path, &expected, "test").unwrap();
    assert_eq!(
        store.plan(plan.id).unwrap().hold_reason.as_deref(),
        Some("waiting on review")
    );
    assert_eq!(store.task(task.id).unwrap().title, "renamed");
    // The legacy singleton still answers active-plan reads for any actor.
    assert_eq!(
        store.meta().unwrap().active_plan_for(None),
        meta.active_plan
    );
}

fn stored_schema(store: &ProjectStore, collection: Collection, id: u64) -> u32 {
    store
        .read(|transaction| {
            Ok(transaction
                .get(collection, RecordKey::Id(id))?
                .expect("record exists")
                .payload_schema())
        })
        .unwrap()
}

#[test]
fn schema_1_records_read_unheld_upgrade_on_write_and_future_schemas_fail_closed() {
    let plan = Plan {
        id: 1,
        title: "legacy".to_owned(),
        status: PlanStatus::Active,
        milestone_id: 0,
        order: 0,
        created_at: timestamp(1_700_000_000),
        updated_at: timestamp(1_700_000_000),
        hold_reason: None,
        actor: None,
        claim_conflict: false,
        claim_epoch: 0,
        claim_owner: None,
        ulid: None,
    };
    // An older build stored the schema-1 layout, under payload schema 1.
    let payload =
        encode_record_at_schema(&NativeRecord::Plan(plan.clone()), MIN_NATIVE_PAYLOAD_SCHEMA)
            .unwrap();
    let legacy = RecordEnvelope::new(NATIVE_CODEC, MIN_NATIVE_PAYLOAD_SCHEMA, payload);

    // It still decodes, and reads as not held.
    assert_eq!(typed::decode::<Plan>(legacy.clone()).unwrap(), plan);

    // Re-encoding lands at the current schema, so any write upgrades the record
    // in place without the open path ever touching it.
    let upgraded = typed::encode(&plan).unwrap();
    assert_eq!(upgraded.payload_schema(), NATIVE_PAYLOAD_SCHEMA);
    assert!(upgraded.payload().len() > legacy.payload().len());
    assert_eq!(typed::decode::<Plan>(upgraded).unwrap(), plan);

    // A schema this build does not know fails closed on read instead of being
    // decoded at the current layout.
    let future = RecordEnvelope::new(
        NATIVE_CODEC,
        NATIVE_PAYLOAD_SCHEMA + 1,
        legacy.payload().to_vec(),
    );
    let error = typed::decode::<Plan>(future).unwrap_err();
    assert!(
        error.to_string().contains("payload schema"),
        "expected a schema error, got {error}"
    );

    // One acceptance rule governs reads, opens, imports, and writes, so a
    // schema-1 envelope can be stored as well as read. Nothing in the
    // application writes one — `typed::encode` always stamps the current
    // schema — but import replays archives byte for byte and must not be
    // refused for carrying the schema its exporter wrote.
    let temp = Temp::new();
    let path = temp.path("lazy-upgrade.redb");
    let expected = binding(&path, StoreKind::Project, "project-upgrade");
    let store = ProjectStore::create_new_with_clock(&path, expected, "test", clock()).unwrap();
    store
        .write(|transaction| {
            transaction.put(Collection::Plans, RecordKey::Id(plan.id), &legacy)?;
            Ok(())
        })
        .unwrap();

    // A schema outside the accepted range is still refused at the write gate.
    let refused = store
        .write(|transaction| {
            let future = RecordEnvelope::new(
                NATIVE_CODEC,
                NATIVE_PAYLOAD_SCHEMA + 1,
                legacy.payload().to_vec(),
            );
            transaction.put(Collection::Plans, RecordKey::Id(plan.id), &future)?;
            Ok(())
        })
        .unwrap_err();
    assert!(matches!(refused, StoreError::InvalidImport(_)), "{refused}");
}

const ACTOR_A: &str = "01hzvyekq3s7m8w9x0abcdefgh";

fn actor_a() -> ActorIdentity {
    ActorIdentity {
        id: ACTOR_A.to_owned(),
        name: "Alice".to_owned(),
    }
}

#[test]
fn mutations_stamp_the_configured_actor_and_register_it() {
    let temp = Temp::new();
    let path = temp.path("actor-stamp.redb");
    let expected = binding(&path, StoreKind::Project, "project-actor");
    let store = ProjectStore::create_new_with_clock(&path, expected, "test", clock())
        .unwrap()
        .with_actor(Some(actor_a()));
    let plan = store.add_plan("attributed", 0).unwrap();
    assert_eq!(plan.actor.as_deref(), Some(ACTOR_A));
    let task = store.add_task(plan.id, "attributed task").unwrap();
    assert_eq!(task.actor.as_deref(), Some(ACTOR_A));
    store.set_task_title(task.id, "renamed").unwrap();
    assert_eq!(store.task(task.id).unwrap().actor.as_deref(), Some(ACTOR_A));
    let meta = store.meta().unwrap();
    assert_eq!(meta.actor_name(ACTOR_A), Some("Alice"));
}

#[test]
fn unset_actor_leaves_records_unattributed() {
    let temp = Temp::new();
    let path = temp.path("actor-unset.redb");
    let expected = binding(&path, StoreKind::Project, "project-noactor");
    let store = ProjectStore::create_new_with_clock(&path, expected, "test", clock()).unwrap();
    let plan = store.add_plan("legacy write", 0).unwrap();
    assert_eq!(plan.actor, None);
    assert!(store.meta().unwrap().actors.is_empty());
}

const ACTOR_B: &str = "01hzvyekq3s7m8w9x0abcdefgj";

fn actor_b() -> ActorIdentity {
    ActorIdentity {
        id: ACTOR_B.to_owned(),
        name: "Bob".to_owned(),
    }
}

#[test]
fn active_plan_is_per_actor_with_legacy_singleton_fallback() {
    let temp = Temp::new();
    let path = temp.path("per-actor-active.redb");
    let expected = binding(&path, StoreKind::Project, "project-per-actor");
    let store = ProjectStore::create_new_with_clock(&path, expected, "test", clock()).unwrap();
    let one = store.add_plan("plan one", 0).unwrap();
    let two = store.add_plan("plan two", 0).unwrap();

    // Legacy path: no identity configured writes the singleton.
    store.set_active_plan(one.id).unwrap();
    assert_eq!(store.snapshot().unwrap().meta.active_plan, one.id);
    drop(store);

    // Actor A picks a different plan; only A sees it.
    let expected = binding(&path, StoreKind::Project, "project-per-actor");
    let store = ProjectStore::open_existing(&path, &expected, "test")
        .unwrap()
        .with_actor(Some(actor_a()));
    store.set_active_plan(two.id).unwrap();
    assert_eq!(store.snapshot().unwrap().meta.active_plan, two.id);
    // The stored singleton is untouched.
    assert_eq!(store.meta().unwrap().active_plan, one.id);
    drop(store);

    // Actor B has no entry yet and falls back to the legacy singleton.
    let expected = binding(&path, StoreKind::Project, "project-per-actor");
    let store = ProjectStore::open_existing(&path, &expected, "test")
        .unwrap()
        .with_actor(Some(actor_b()));
    assert_eq!(store.snapshot().unwrap().meta.active_plan, one.id);
    // An explicit zero entry means "none", not "fall back".
    store.set_active_plan(0).unwrap();
    assert_eq!(store.snapshot().unwrap().meta.active_plan, 0);
}

fn reopen_as(path: &std::path::Path, label: &str, actor: Option<ActorIdentity>) -> ProjectStore {
    let expected = binding(path, StoreKind::Project, label);
    ProjectStore::open_existing(path, &expected, "test")
        .unwrap()
        .with_actor(actor)
}

#[test]
fn claims_gate_content_mutations_but_not_holds_notes_or_issues() {
    let temp = Temp::new();
    let path = temp.path("claims.redb");
    let expected = binding(&path, StoreKind::Project, "project-claims");
    let store = ProjectStore::create_new_with_clock(&path, expected, "test", clock()).unwrap();
    let plan = store.add_plan("shared", 0).unwrap();
    let task = store.add_task(plan.id, "shared task").unwrap();
    drop(store);

    // A claims the plan.
    let store = reopen_as(&path, "project-claims", Some(actor_a()));
    store.use_plan(plan.id, false).unwrap();
    let claimed = store.plan(plan.id).unwrap();
    assert_eq!(claimed.claim_owner.as_deref(), Some(ACTOR_A));
    assert_eq!(claimed.claim_epoch, 1);
    // Re-using your own claim is idempotent: no epoch bump.
    store.use_plan(plan.id, false).unwrap();
    assert_eq!(store.plan(plan.id).unwrap().claim_epoch, 1);
    drop(store);

    // B: content mutations refused, communication channels open.
    let store = reopen_as(&path, "project-claims", Some(actor_b()));
    let refused = store.set_plan_title(plan.id, "hijack").unwrap_err();
    assert!(matches!(refused, StoreError::InvalidClaim(_)), "{refused}");
    assert!(refused.to_string().starts_with(INVALID_CLAIM_PREFIX));
    assert!(matches!(
        store
            .set_task_status(task.id, TaskStatus::Doing)
            .unwrap_err(),
        StoreError::InvalidClaim(_)
    ));
    assert!(matches!(
        store.add_task(plan.id, "b task").unwrap_err(),
        StoreError::InvalidClaim(_)
    ));
    store
        .set_plan_hold(plan.id, Some("please pause".to_owned()))
        .unwrap();
    store.set_task_hold(task.id, None).unwrap();
    store
        .add_note(NoteTarget::Plan, plan.id, "note from B")
        .unwrap();
    store.add_issue("issue from B", "", None, task.id).unwrap();
    // Non-steal takeover refused; steal succeeds and bumps the epoch.
    assert!(matches!(
        store.use_plan(plan.id, false).unwrap_err(),
        StoreError::InvalidClaim(_)
    ));
    store.use_plan(plan.id, true).unwrap();
    let stolen = store.plan(plan.id).unwrap();
    assert_eq!(stolen.claim_owner.as_deref(), Some(ACTOR_B));
    assert_eq!(stolen.claim_epoch, 2);
    // Release frees the claim, keeps the epoch, and only the owner may do it.
    store.release_plan(plan.id).unwrap();
    let released = store.plan(plan.id).unwrap();
    assert_eq!(released.claim_owner, None);
    assert_eq!(released.claim_epoch, 2);
    assert!(matches!(
        store.release_plan(plan.id).unwrap_err(),
        StoreError::InvalidClaim(_)
    ));
}

#[test]
fn unset_actor_cannot_mutate_claimed_content_and_terminal_status_releases() {
    let temp = Temp::new();
    let path = temp.path("claims-terminal.redb");
    let expected = binding(&path, StoreKind::Project, "project-terminal");
    let store = ProjectStore::create_new_with_clock(&path, expected, "test", clock()).unwrap();
    let plan = store.add_plan("terminal", 0).unwrap();
    drop(store);

    let store = reopen_as(&path, "project-terminal", Some(actor_a()));
    store.use_plan(plan.id, false).unwrap();
    drop(store);

    // No identity configured: claimed content is refused fail-closed.
    let store = reopen_as(&path, "project-terminal", None);
    assert!(matches!(
        store.set_plan_title(plan.id, "anon edit").unwrap_err(),
        StoreError::InvalidClaim(_)
    ));
    drop(store);

    // The owner finishing the plan auto-releases the claim.
    let store = reopen_as(&path, "project-terminal", Some(actor_a()));
    store.set_plan_status(plan.id, PlanStatus::Done).unwrap();
    let done = store.plan(plan.id).unwrap();
    assert_eq!(done.claim_owner, None);
    assert_eq!(done.claim_epoch, 1);
}

#[test]
fn convert_births_the_new_plan_claimed_by_the_converter() {
    let temp = Temp::new();
    let path = temp.path("claims-convert.redb");
    let expected = binding(&path, StoreKind::Project, "project-convert");
    let store = ProjectStore::create_new_with_clock(&path, expected, "test", clock()).unwrap();
    let plan = store.add_plan("parent", 0).unwrap();
    let task = store.add_task(plan.id, "promote me").unwrap();
    drop(store);

    let store = reopen_as(&path, "project-convert", Some(actor_a()));
    store.use_plan(plan.id, false).unwrap();
    let born = store.convert_task_to_plan(task.id).unwrap();
    assert_eq!(born.claim_owner.as_deref(), Some(ACTOR_A));
    assert_eq!(born.claim_epoch, 1);
}

/// Marks a plan's claim as conflicted. Nothing in the store writes this field
/// — it is accepted-but-never-written until conflict detection lands — so the
/// only way to prove the clearing paths handle it is to plant one.
fn plant_claim_conflict(store: &ProjectStore, plan_id: u64) {
    store
        .write(|transaction| {
            let mut plan: Plan = typed::get_write(transaction, RecordKey::Id(plan_id))?.unwrap();
            plan.claim_conflict = true;
            typed::put(transaction, RecordKey::Id(plan_id), &plan)?;
            Ok(())
        })
        .unwrap();
}

#[test]
fn claim_gate_covers_the_compare_and_set_and_cross_plan_paths() {
    let temp = Temp::new();
    let path = temp.path("claims-gates.redb");
    let expected = binding(&path, StoreKind::Project, "project-gates");
    let store = ProjectStore::create_new_with_clock(&path, expected, "test", clock()).unwrap();
    let source = store.add_plan("source", 0).unwrap();
    let target = store.add_plan("target", 0).unwrap();
    let task = store.add_task(source.id, "moving target").unwrap();
    drop(store);

    let store = reopen_as(&path, "project-gates", Some(actor_a()));
    store.use_plan(source.id, false).unwrap();
    drop(store);

    // B owns nothing here: the CAS path is gated like every other content write.
    let store = reopen_as(&path, "project-gates", Some(actor_b()));
    let refused = store
        .compare_and_set_task_status(
            task.id,
            source.id,
            TaskStatus::Todo,
            task.updated_at,
            TaskStatus::Doing,
        )
        .unwrap_err();
    assert!(matches!(refused, StoreError::InvalidClaim(_)), "{refused}");
    // The refusal names the owner the way a person knows them, not by ULID.
    assert!(refused.to_string().contains("Alice"), "{refused}");
    assert!(!refused.to_string().contains(ACTOR_A), "{refused}");

    // Only the *source* plan is claimed, and moving out of it is still refused.
    assert!(matches!(
        store.set_task_plan(task.id, target.id).unwrap_err(),
        StoreError::InvalidClaim(_)
    ));
    // The open channels leave the claim exactly as they found it.
    store
        .set_plan_hold(source.id, Some("waiting on B".to_owned()))
        .unwrap();
    store.set_plan_hold(source.id, None).unwrap();
    let held = store.plan(source.id).unwrap();
    assert_eq!(held.claim_owner.as_deref(), Some(ACTOR_A));
    assert_eq!(held.claim_epoch, 1);
    drop(store);

    // Finishing a *task* is not a plan status change: the claim is untouched.
    let store = reopen_as(&path, "project-gates", Some(actor_a()));
    let task = store
        .compare_and_set_task_status(
            task.id,
            source.id,
            TaskStatus::Todo,
            task.updated_at,
            TaskStatus::Done,
        )
        .unwrap();
    assert_eq!(task.status, TaskStatus::Done);
    assert_eq!(
        store.plan(source.id).unwrap().claim_owner.as_deref(),
        Some(ACTOR_A)
    );
}

#[test]
fn claim_epochs_move_only_on_a_real_ownership_change() {
    let temp = Temp::new();
    let path = temp.path("claims-epoch.redb");
    let expected = binding(&path, StoreKind::Project, "project-epoch");
    let store = ProjectStore::create_new_with_clock(&path, expected, "test", clock()).unwrap();
    let plan = store.add_plan("epochs", 0).unwrap();
    drop(store);

    let store = reopen_as(&path, "project-epoch", Some(actor_a()));
    // Stealing a plan nobody holds is an ordinary first claim.
    store.use_plan(plan.id, true).unwrap();
    assert_eq!(store.plan(plan.id).unwrap().claim_epoch, 1);
    // Stealing from yourself changes no ownership, so it bumps nothing.
    store.use_plan(plan.id, true).unwrap();
    assert_eq!(store.plan(plan.id).unwrap().claim_epoch, 1);
    drop(store);

    // An identity-less caller still moves its legacy active plan and leaves
    // the claim alone, even though its content writes are refused.
    let store = reopen_as(&path, "project-epoch", None);
    store.use_plan(plan.id, false).unwrap();
    assert_eq!(store.meta().unwrap().active_plan, plan.id);
    let untouched = store.plan(plan.id).unwrap();
    assert_eq!(untouched.claim_owner.as_deref(), Some(ACTOR_A));
    assert_eq!(untouched.claim_epoch, 1);
    // `--steal` is meaningless without an identity to steal for.
    assert!(matches!(
        store.use_plan(plan.id, true).unwrap_err(),
        StoreError::InvalidClaim(_)
    ));
    drop(store);

    // Terminal auto-release is a release: a second, explicit one has nothing
    // left to free.
    let store = reopen_as(&path, "project-epoch", Some(actor_a()));
    store.set_plan_status(plan.id, PlanStatus::Done).unwrap();
    assert!(matches!(
        store.release_plan(plan.id).unwrap_err(),
        StoreError::InvalidClaim(_)
    ));
}

#[test]
fn terminal_plans_hold_no_claim_from_any_direction() {
    let temp = Temp::new();
    let path = temp.path("claims-terminal-open.redb");
    let expected = binding(&path, StoreKind::Project, "project-terminal-open");
    let store = ProjectStore::create_new_with_clock(&path, expected, "test", clock()).unwrap();
    let plan = store.add_plan("finished", 0).unwrap();
    let done_task = store.add_task(plan.id, "already done").unwrap();
    store
        .set_task_status(done_task.id, TaskStatus::Done)
        .unwrap();
    drop(store);

    let store = reopen_as(&path, "project-terminal-open", Some(actor_a()));
    store.use_plan(plan.id, false).unwrap();
    // A done task births a done plan, and a done plan is born unclaimed.
    let born = store.convert_task_to_plan(done_task.id).unwrap();
    assert_eq!(born.status, PlanStatus::Done);
    assert_eq!(born.claim_owner, None);
    assert_eq!(born.claim_epoch, 0);
    store.set_plan_status(plan.id, PlanStatus::Done).unwrap();
    drop(store);

    // Using or stealing a terminal plan moves the active plan and claims
    // nothing, so its content stays open to everyone.
    let store = reopen_as(&path, "project-terminal-open", Some(actor_b()));
    store.use_plan(plan.id, true).unwrap();
    assert_eq!(store.plan(plan.id).unwrap().claim_owner, None);
    assert_eq!(store.snapshot().unwrap().meta.active_plan, plan.id);
    store
        .set_plan_title(plan.id, "renamed after the fact")
        .unwrap();
    assert_eq!(store.plan(born.id).unwrap().claim_owner, None);
}

#[test]
fn a_conflicted_claim_can_still_be_stolen_released_and_finished() {
    let temp = Temp::new();
    let path = temp.path("claims-conflict.redb");
    let expected = binding(&path, StoreKind::Project, "project-conflict");
    let store = ProjectStore::create_new_with_clock(&path, expected, "test", clock()).unwrap();
    let released = store.add_plan("released", 0).unwrap();
    let finished = store.add_plan("finished", 0).unwrap();
    let stolen = store.add_plan("stolen", 0).unwrap();
    drop(store);

    let store = reopen_as(&path, "project-conflict", Some(actor_a()));
    for plan in [&released, &finished, &stolen] {
        store.use_plan(plan.id, false).unwrap();
        plant_claim_conflict(&store, plan.id);
    }

    // Every write that clears an owner clears the marker with it; leaving it
    // behind would make the record fail validation on encode forever.
    store.release_plan(released.id).unwrap();
    assert!(!store.plan(released.id).unwrap().claim_conflict);
    store
        .set_plan_status(finished.id, PlanStatus::Done)
        .unwrap();
    assert!(!store.plan(finished.id).unwrap().claim_conflict);
    drop(store);

    let store = reopen_as(&path, "project-conflict", Some(actor_b()));
    store.use_plan(stolen.id, true).unwrap();
    let taken = store.plan(stolen.id).unwrap();
    assert_eq!(taken.claim_owner.as_deref(), Some(ACTOR_B));
    assert!(!taken.claim_conflict);
}

#[test]
fn delete_plan_cascades_tasks_notes_detaches_issues_and_zeroes_commits() {
    let temp = Temp::new();
    let store = ProjectStore::create_new_with_clock(
        temp.path("delete.redb"),
        binding(&temp.path("delete.redb"), StoreKind::Project, "delete-1"),
        "test",
        clock(),
    )
    .unwrap();
    let doomed = store.add_plan("Doomed", 0).unwrap();
    let survivor = store.add_plan("Survivor", 0).unwrap();
    let task = store.add_task(doomed.id, "dead task").unwrap();
    let kept_task = store.add_task(survivor.id, "kept task").unwrap();
    store
        .add_note(NoteTarget::Plan, doomed.id, "plan note")
        .unwrap();
    store
        .add_note(NoteTarget::Task, task.id, "task note")
        .unwrap();
    store
        .add_note(NoteTarget::Task, kept_task.id, "kept note")
        .unwrap();
    let issue = store.add_issue("crash on save", "", None, task.id).unwrap();
    store
        .add_commit("aaa111", "linked", doomed.id, task.id)
        .unwrap();
    store.add_commit("bbb222", "kept", survivor.id, 0).unwrap();
    store.set_active_plan(doomed.id).unwrap();

    let summary: PlanDeleteSummary = store.delete_plan(doomed.id).unwrap();
    assert_eq!(summary.plan_id, doomed.id);
    assert_eq!(summary.title, "Doomed");
    assert_eq!(summary.tasks, 1);
    assert_eq!(summary.notes, 2);
    assert_eq!(summary.commits_unlinked, 1);
    assert_eq!(summary.issues, vec![(issue.id, "crash on save".to_owned())]);

    // Sweep every collection: nothing references the dead plan or its task.
    let snapshot = store.snapshot().unwrap();
    assert!(snapshot.plans.iter().all(|plan| plan.id != doomed.id));
    assert!(
        snapshot
            .tasks
            .iter()
            .all(|task_| task_.plan_id != doomed.id)
    );
    assert!(
        snapshot
            .notes
            .iter()
            .all(
                |note| !(note.target == NoteTarget::Task && note.target_id == task.id)
                    && !(note.target == NoteTarget::Plan && note.target_id == doomed.id)
            )
    );
    let detached = snapshot.issues.iter().find(|i| i.id == issue.id).unwrap();
    assert_eq!(detached.task_id, 0);
    let unlinked = snapshot
        .commits
        .iter()
        .find(|commit| commit.sha == "aaa111")
        .unwrap();
    assert_eq!((unlinked.plan_id, unlinked.task_id), (0, 0));
    let kept = snapshot
        .commits
        .iter()
        .find(|commit| commit.sha == "bbb222")
        .unwrap();
    assert_eq!(kept.plan_id, survivor.id);
    // Legacy active-plan singleton reset to 0.
    assert_eq!(store.meta().unwrap().active_plan, 0);
}

#[test]
fn delete_plan_resets_every_actor_pointer_and_respects_claims() {
    let temp = Temp::new();
    let path = temp.path("delete-claims.redb");
    let expected = binding(&path, StoreKind::Project, "delete-claims-1");
    let alice = ProjectStore::create_new_with_clock(&path, expected.clone(), "test", clock())
        .unwrap()
        .with_actor(Some(ActorIdentity {
            id: "01hzvyekq3s7m8w9x0aaaaaaaa".to_owned(),
            name: "Alice".to_owned(),
        }));
    let bob = ProjectStore::open_existing(&path, &expected, "test")
        .unwrap()
        .with_actor(Some(ActorIdentity {
            id: "01hzvyekq3s7m8w9x0bbbbbbbb".to_owned(),
            name: "Bob".to_owned(),
        }));
    let plan = alice.add_plan("Claimed", 0).unwrap();
    alice.use_plan(plan.id, false).unwrap();
    bob.use_plan(plan.id, true).unwrap(); // Bob steals and points at it too.
    alice.use_plan(plan.id, true).unwrap(); // Alice steals back; both actors point at it.

    // Bob cannot delete Alice's claimed plan.
    let refusal = bob.delete_plan(plan.id).unwrap_err();
    assert!(refusal.to_string().starts_with(INVALID_CLAIM_PREFIX));

    // The owner's own claim dies with the plan; every pointer resets to 0.
    alice.delete_plan(plan.id).unwrap();
    let meta = alice.meta().unwrap();
    assert_eq!(meta.active_plan, 0);
    assert!(meta.active_plans.iter().all(|(_, plan_id)| *plan_id == 0));
    assert!(matches!(alice.plan(plan.id), Err(StoreError::NotFound)));
}

#[test]
fn delete_plan_for_move_deletes_linked_issues_instead_of_detaching() {
    let temp = Temp::new();
    let store = ProjectStore::create_new_with_clock(
        temp.path("move-delete.redb"),
        binding(
            &temp.path("move-delete.redb"),
            StoreKind::Project,
            "move-delete-1",
        ),
        "test",
        clock(),
    )
    .unwrap();
    let plan = store.add_plan("Moving out", 0).unwrap();
    let task = store.add_task(plan.id, "t").unwrap();
    let issue = store
        .add_issue("follows its task", "", None, task.id)
        .unwrap();
    let unrelated = store.add_issue("stays", "", None, 0).unwrap();

    let summary = store.delete_plan_for_move(plan.id).unwrap();
    assert_eq!(
        summary.issues,
        vec![(issue.id, "follows its task".to_owned())]
    );
    let snapshot = store.snapshot().unwrap();
    assert!(snapshot.issues.iter().all(|i| i.id != issue.id)); // moved, not detached
    assert!(snapshot.issues.iter().any(|i| i.id == unrelated.id)); // untouched
}

#[test]
fn plan_delete_preview_counts_without_mutating() {
    let temp = Temp::new();
    let store = ProjectStore::create_new_with_clock(
        temp.path("preview.redb"),
        binding(&temp.path("preview.redb"), StoreKind::Project, "preview-1"),
        "test",
        clock(),
    )
    .unwrap();
    let plan = store.add_plan("Previewed", 0).unwrap();
    let task = store.add_task(plan.id, "one").unwrap();
    store.add_note(NoteTarget::Task, task.id, "note").unwrap();
    store.add_issue("bug", "", None, task.id).unwrap();

    let summary = store.plan_delete_preview(plan.id).unwrap();
    assert_eq!((summary.tasks, summary.notes), (1, 1));
    assert_eq!(summary.issues.len(), 1);
    assert_eq!(summary.commits_unlinked, 0);
    // Nothing changed.
    assert_eq!(store.snapshot().unwrap().tasks.len(), 1);
    assert!(matches!(
        store.plan_delete_preview(9999),
        Err(StoreError::NotFound)
    ));
}

#[test]
fn delete_plan_removes_dangling_memory_writeback_receipts() {
    let temp = Temp::new();
    let path = temp.path("memory-delete.redb");
    let store = ProjectStore::create_new_with_clock(
        &path,
        binding(&path, StoreKind::Project, "memory-delete-1"),
        "test",
        clock(),
    )
    .unwrap();
    let plan = store.add_plan("Doomed", 0).unwrap();
    let request = MemoryWriteRequest {
        request_id: "req-memory-1".to_owned(),
        kind: MemoryKind::Decision,
        body: "remember this".to_owned(),
        target: NoteTarget::Plan,
        target_id: plan.id,
        plan_id: plan.id,
        workspace_generation: 7,
        session_id: "session".to_owned(),
        association_revision: 1,
    };
    let result = store.write_memory(request.clone()).unwrap();
    assert!(!result.replayed);
    let note_id = result.note.unwrap().id;

    let receipt_present = |store: &ProjectStore| {
        store
            .read(|transaction| {
                Ok(transaction
                    .get(
                        Collection::MemoryWritebacks,
                        RecordKey::Bytes(b"req-memory-1"),
                    )?
                    .is_some())
            })
            .unwrap()
    };
    assert!(receipt_present(&store));

    store.delete_plan(plan.id).unwrap();
    assert!(!receipt_present(&store));
    assert!(
        store
            .snapshot()
            .unwrap()
            .notes
            .iter()
            .all(|note| note.id != note_id)
    );

    // With the receipt gone, replaying the same request_id is a fresh
    // request rather than the old bare NotFound from a receipt pointing at
    // a deleted note: validation reports the plan target itself is gone.
    assert!(matches!(
        store.write_memory(request),
        Err(StoreError::InvalidMemoryWriteback(_))
    ));
}

#[test]
fn delete_plan_leaves_a_surviving_plans_issue_untouched() {
    let temp = Temp::new();
    let store = ProjectStore::create_new_with_clock(
        temp.path("isolation.redb"),
        binding(
            &temp.path("isolation.redb"),
            StoreKind::Project,
            "isolation-1",
        ),
        "test",
        clock(),
    )
    .unwrap();
    let doomed = store.add_plan("Doomed", 0).unwrap();
    let survivor = store.add_plan("Survivor", 0).unwrap();
    let doomed_task = store.add_task(doomed.id, "dies").unwrap();
    let kept_task = store.add_task(survivor.id, "lives").unwrap();
    let doomed_issue = store
        .add_issue("dies with its task", "", None, doomed_task.id)
        .unwrap();
    let kept_issue = store
        .add_issue("belongs to survivor", "", None, kept_task.id)
        .unwrap();

    let summary = store.delete_plan(doomed.id).unwrap();
    assert_eq!(
        summary.issues,
        vec![(doomed_issue.id, doomed_issue.title.clone())]
    );

    let snapshot = store.snapshot().unwrap();
    let untouched = snapshot
        .issues
        .iter()
        .find(|issue| issue.id == kept_issue.id)
        .unwrap();
    assert_eq!(untouched.task_id, kept_task.id);
    let detached = snapshot
        .issues
        .iter()
        .find(|issue| issue.id == doomed_issue.id)
        .unwrap();
    assert_eq!(detached.task_id, 0);
}

#[test]
fn export_import_copies_a_plan_subtree_with_reminted_ids_and_no_dangling_refs() {
    let temp = Temp::new();
    let store = ProjectStore::create_new_with_clock(
        temp.path("copy.redb"),
        binding(&temp.path("copy.redb"), StoreKind::Project, "copy-1"),
        "test",
        clock(),
    )
    .unwrap();
    let milestone = store.add_milestone("M1").unwrap();
    let plan = store.add_plan("Original", milestone.id).unwrap();
    store
        .set_plan_hold(plan.id, Some("waiting".to_owned()))
        .unwrap();
    let task = store.add_task(plan.id, "t1").unwrap();
    store
        .add_note(NoteTarget::Plan, plan.id, "plan note")
        .unwrap();
    store
        .add_note(NoteTarget::Task, task.id, "task note")
        .unwrap();
    let issue = store.add_issue("bug", "", None, task.id).unwrap();
    store
        .add_commit("ccc333", "work", plan.id, task.id)
        .unwrap();

    let subtree = store.export_plan_subtree(plan.id).unwrap();
    assert_eq!(subtree.tasks.len(), 1);
    assert_eq!(subtree.notes.len(), 2);
    assert_eq!(subtree.issues.len(), 1);
    assert_eq!(subtree.commits.len(), 1);

    let copy = store
        .import_plan_subtree(&subtree, Some("Copied".to_owned()))
        .unwrap();
    assert_ne!(copy.id, plan.id);
    assert_eq!(copy.title, "Copied");
    assert_eq!(copy.milestone_id, 0); // milestone link dropped
    assert_eq!(copy.hold_reason.as_deref(), Some("waiting")); // hold travels
    assert_eq!(copy.claim_owner, None); // arrives unclaimed
    assert_eq!(copy.claim_epoch, 0);

    let snapshot = store.snapshot().unwrap();
    let copied_tasks: Vec<_> = snapshot
        .tasks
        .iter()
        .filter(|t| t.plan_id == copy.id)
        .collect();
    assert_eq!(copied_tasks.len(), 1);
    let copied_task = copied_tasks[0];
    assert_ne!(copied_task.id, task.id);
    // Every copied reference points at reminted IDs — zero dangling.
    let copied_issue = snapshot.issues.iter().find(|i| i.id != issue.id).unwrap();
    assert_eq!(copied_issue.task_id, copied_task.id);
    let copied_commit = snapshot
        .commits
        .iter()
        .find(|c| c.sha == "ccc333" && c.plan_id == copy.id)
        .unwrap();
    assert_eq!(copied_commit.task_id, copied_task.id);
    assert!(
        snapshot
            .notes
            .iter()
            .any(|n| n.target == NoteTarget::Task && n.target_id == copied_task.id)
    );
    assert!(
        snapshot
            .notes
            .iter()
            .any(|n| n.target == NoteTarget::Plan && n.target_id == copy.id)
    );

    // The copy is independent: mutating it leaves the original untouched.
    store
        .set_task_status(copied_task.id, TaskStatus::Done)
        .unwrap();
    let after = store.snapshot().unwrap();
    assert_eq!(
        after.tasks.iter().find(|t| t.id == task.id).unwrap().status,
        TaskStatus::Todo
    );
    assert_eq!(
        after.plans.iter().find(|p| p.id == plan.id).unwrap().title,
        "Original"
    );
}

#[test]
fn export_plan_subtree_is_claim_gated() {
    let temp = Temp::new();
    let path = temp.path("export-gate.redb");
    let expected = binding(&path, StoreKind::Project, "export-gate-1");
    let alice = ProjectStore::create_new_with_clock(&path, expected.clone(), "test", clock())
        .unwrap()
        .with_actor(Some(ActorIdentity {
            id: "01hzvyekq3s7m8w9x0aaaaaaaa".to_owned(),
            name: "Alice".to_owned(),
        }));
    let bob = ProjectStore::open_existing(&path, &expected, "test")
        .unwrap()
        .with_actor(Some(ActorIdentity {
            id: "01hzvyekq3s7m8w9x0bbbbbbbb".to_owned(),
            name: "Bob".to_owned(),
        }));
    let plan = alice.add_plan("Mine", 0).unwrap();
    alice.use_plan(plan.id, false).unwrap();
    let refusal = bob.export_plan_subtree(plan.id).unwrap_err();
    assert!(refusal.to_string().starts_with(INVALID_CLAIM_PREFIX));
    assert!(alice.export_plan_subtree(plan.id).is_ok());
}
