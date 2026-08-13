use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

use ptrack_capability_policy::{AuditEvent, confirm_approval, normalize, sanitize_audit};
use ptrack_core::{
    Capability, CapabilityAudit, CapabilityAuditPolicy, CapabilityKind, CapabilityLimits, Digest32,
    GitScope, MemoryKind, NativeRecord, NoteTarget, RecordKind, TaskStatus, Timestamp,
    decode_record,
};

use crate::{
    ActiveBinding, Clock, Collection, GlobalStore, MemoryWriteRequest, ProjectStore, RecordKey,
    StoreError, StoreKind,
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
    assert!(store.application_writes().unwrap());

    let milestone = store.add_milestone("m").unwrap();
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
