use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicI64, AtomicU8, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Barrier};
use std::time::Duration;

use super::*;
use crate::test_support::TempDirectory;

struct Catalog;

impl AssociationCatalog for Catalog {
    fn validate_plan(&self, plan_id: u64) -> Result<(), String> {
        (plan_id == 7)
            .then_some(())
            .ok_or_else(|| "not found".to_owned())
    }

    fn task_plan(&self, task_id: u64) -> Result<u64, String> {
        BTreeMap::from([(9, 7)])
            .get(&task_id)
            .copied()
            .ok_or_else(|| "not found".to_owned())
    }
}

fn config(root: &Path, seconds: Arc<AtomicI64>) -> RegistryConfig {
    let random_counter = Arc::new(AtomicU8::new(0));
    RegistryConfig {
        project_root: root.to_path_buf(),
        now: Some(Arc::new(move || {
            Timestamp::from_unix_seconds(seconds.load(Ordering::SeqCst))
        })),
        random: Some(Arc::new(move |bytes| {
            let value = random_counter.fetch_add(1, Ordering::SeqCst);
            bytes.fill(value);
            Ok(())
        })),
        ..RegistryConfig::default()
    }
}

fn registration(kind: RegistrationKind) -> Registration {
    Registration {
        profile: " profile ".to_owned(),
        provider: " codex ".to_owned(),
        pid: if kind == RegistrationKind::Launched {
            42
        } else {
            0
        },
        terminal_id: if kind == RegistrationKind::Launched {
            "terminal-1".to_owned()
        } else {
            String::new()
        },
        cwd: String::new(),
    }
}

fn observation(source_id: &str, sequence: u64) -> EventObservation {
    EventObservation {
        model_version: EVENT_MODEL_VERSION,
        source_id: source_id.to_owned(),
        source_sequence: sequence,
        kind: EventKind::Tool,
        phase: EventPhase::Progress,
        subject: "compile".to_owned(),
        ..EventObservation::default()
    }
}

#[test]
fn defaults_registration_json_and_opaque_values_match_go_contract() {
    let root = TempDirectory::new("ptrack-agent-registry-defaults");
    let now = Arc::new(AtomicI64::new(100));
    let registry = Registry::new(config(root.path(), now));
    let lease = registry
        .register_external(registration(RegistrationKind::External))
        .unwrap();
    assert_eq!(lease.run.profile, "profile");
    assert_eq!(lease.run.provider, "codex");
    assert_eq!(lease.run.id.len(), 43);
    assert_eq!(lease.lease_token.len(), 43);
    assert!(!lease.run.id.contains('='));
    let lease_debug = format!("{lease:?}");
    assert!(lease_debug.contains("[redacted]"));
    assert!(!lease_debug.contains(&lease.lease_token));
    assert_eq!(lease.run.process_state, ProcessState::Unknown);
    assert_eq!(lease.run.lease_state, LeaseState::Active);
    assert_eq!(lease.run.last_heartbeat_at, lease.run.started_at);
    assert_eq!(registry.active_count(), 1);

    let launched = registry
        .register_launched(registration(RegistrationKind::Launched))
        .unwrap();
    assert_eq!(launched.process_state, ProcessState::Running);
    assert_eq!(launched.lease_state, LeaseState::None);
    assert!(launched.last_heartbeat_at.is_zero());
    let json = serde_json::to_value(&launched).unwrap();
    assert_eq!(json["registrationKind"], "launched");
    assert!(json.get("lifecycleRevision").is_none());
}

#[test]
fn registration_validation_canonical_cwd_and_external_authority_fail_closed() {
    let root = TempDirectory::new("ptrack-agent-registry-cwd");
    let outside = TempDirectory::new("ptrack-agent-registry-outside");
    let now = Arc::new(AtomicI64::new(100));
    let mut cfg = config(root.path(), now);
    cfg.additional_cwd_validator = Some(Arc::new(|_| true));
    let registry = Registry::new(cfg);
    assert_eq!(
        registry
            .register_launched(Registration::default())
            .unwrap_err()
            .to_string(),
        "launched AgentRun requires PID and terminal"
    );
    let mut missing = registration(RegistrationKind::External);
    missing.profile.clear();
    assert_eq!(
        registry.register_external(missing).unwrap_err().to_string(),
        "AgentRun profile and provider are required"
    );
    let mut external = registration(RegistrationKind::External);
    external.cwd = outside.path().to_string_lossy().into_owned();
    assert_eq!(
        registry
            .register_external(external)
            .unwrap_err()
            .to_string(),
        "AgentRun CWD is outside the project"
    );
    let mut launched = registration(RegistrationKind::Launched);
    launched.cwd = outside.path().to_string_lossy().into_owned();
    assert_eq!(
        registry.register_launched(launched).unwrap().cwd,
        std::fs::canonicalize(outside.path())
            .unwrap()
            .to_string_lossy()
    );
}

#[test]
fn external_lease_sweep_revival_exit_and_exact_epoch_are_bounded() {
    let root = TempDirectory::new("ptrack-agent-registry-lease");
    let now = Arc::new(AtomicI64::new(100));
    let registry = Registry::new(config(root.path(), Arc::clone(&now)));
    let lease = registry
        .register_external(registration(RegistrationKind::External))
        .unwrap();
    assert_eq!(
        registry.heartbeat(&lease.run.id, "wrong").unwrap_err(),
        RegistryError::InvalidLease
    );
    assert_eq!(
        registry.heartbeat("missing", "wrong").unwrap_err(),
        RegistryError::RunNotFound
    );
    now.store(130, Ordering::SeqCst);
    registry.sweep_expired();
    assert_eq!(
        registry.run(&lease.run.id).unwrap().state,
        RunState::Running
    );
    now.store(131, Ordering::SeqCst);
    registry.sweep_expired();
    assert_eq!(registry.run(&lease.run.id).unwrap().state, RunState::Stale);
    registry
        .heartbeat(&lease.run.id, &lease.lease_token)
        .unwrap();
    registry
        .with_exact_runtime_snapshot(1, |runs| {
            assert_eq!(runs[0].lifecycle_revision, 3);
            Ok(())
        })
        .unwrap();
    now.store(140, Ordering::SeqCst);
    registry
        .exit_external(&lease.run.id, &lease.lease_token, 9, "secret raw output")
        .unwrap();
    let exited = registry.run(&lease.run.id).unwrap();
    assert_eq!(exited.state, RunState::Exited);
    assert_eq!(exited.lease_state, LeaseState::Expired);
    assert_eq!(exited.exit.unwrap().result, "failed");
    assert_eq!(
        registry
            .heartbeat(&lease.run.id, &lease.lease_token)
            .unwrap_err(),
        RegistryError::InvalidLease
    );
}

#[test]
fn admission_fence_capacity_evicts_only_oldest_inactive_and_snapshots_sort() {
    let root = TempDirectory::new("ptrack-agent-registry-capacity");
    let now = Arc::new(AtomicI64::new(100));
    let mut cfg = config(root.path(), Arc::clone(&now));
    cfg.max_records = 2;
    let registry = Registry::new(cfg);
    let fence = registry.fence_admission();
    assert_eq!(
        registry
            .register_external(registration(RegistrationKind::External))
            .unwrap_err(),
        RegistryError::AdmissionFenced
    );
    fence.release();
    let first = registry
        .register_external(registration(RegistrationKind::External))
        .unwrap();
    now.store(101, Ordering::SeqCst);
    let second = registry
        .register_external(registration(RegistrationKind::External))
        .unwrap();
    assert_eq!(
        registry
            .register_external(registration(RegistrationKind::External))
            .unwrap_err(),
        RegistryError::Full
    );
    registry
        .exit_external(&first.run.id, &first.lease_token, 0, "DONE")
        .unwrap();
    now.store(102, Ordering::SeqCst);
    let third = registry
        .register_external(registration(RegistrationKind::External))
        .unwrap();
    assert_eq!(
        registry.run(&first.run.id).unwrap_err(),
        RegistryError::RunNotFound
    );
    let (snapshot, total) = registry.snapshot_bounded(0);
    assert_eq!(total, 2);
    assert_eq!(snapshot[0].id, third.run.id);
    assert_eq!(snapshot[1].id, second.run.id);
    assert_eq!(
        registry
            .with_exact_runtime_snapshot(1, |_| Ok(()))
            .unwrap_err(),
        RegistryError::SnapshotLimit
    );
}

#[test]
fn pending_event_tokens_have_no_authority_and_bind_wakes_waiters() {
    let root = TempDirectory::new("ptrack-agent-registry-token");
    let now = Arc::new(AtomicI64::new(100));
    let registry = Registry::new(config(root.path(), now));
    let run = registry
        .register_launched(registration(RegistrationKind::Launched))
        .unwrap();
    let token = registry.issue_launched_event_token().unwrap();
    assert_eq!(token.len(), 43);
    assert_eq!(
        registry
            .authenticate_launched_event_token(&token)
            .unwrap_err(),
        RegistryError::InvalidEventToken
    );
    std::thread::scope(|scope| {
        let waiter_token = token.clone();
        let registry_ref = &registry;
        let waiter = scope.spawn(move || {
            registry_ref.await_launched_event_token(&waiter_token, Duration::from_secs(1))
        });
        registry.bind_launched_event_token(&token, &run.id).unwrap();
        waiter.join().unwrap().unwrap();
    });
    registry.authenticate_launched_event_token(&token).unwrap();
    registry.revoke_launched_event_token(&token);
    registry.revoke_launched_event_token(&token);
    assert_eq!(
        registry
            .authenticate_launched_event_token(&token)
            .unwrap_err(),
        RegistryError::InvalidEventToken
    );

    let revoked = registry.issue_launched_event_token().unwrap();
    registry.revoke_launched_event_token(&revoked);
    assert_eq!(
        registry
            .await_launched_event_token(&revoked, Duration::from_millis(1))
            .unwrap_err(),
        RegistryError::InvalidEventToken
    );
}

#[test]
fn bind_failure_keeps_pending_token_and_nested_fences_release_exactly_once() {
    let root = TempDirectory::new("ptrack-agent-registry-token-fences");
    let now = Arc::new(AtomicI64::new(100));
    let registry = Registry::new(config(root.path(), now));
    let first_fence = registry.fence_admission();
    let second_fence = registry.fence_admission();
    assert_eq!(
        registry.issue_launched_event_token().unwrap_err(),
        RegistryError::AdmissionFenced
    );
    first_fence.release();
    assert_eq!(
        registry.issue_launched_event_token().unwrap_err(),
        RegistryError::AdmissionFenced
    );
    second_fence.release();
    let token = registry.issue_launched_event_token().unwrap();
    assert_eq!(
        registry
            .bind_launched_event_token(&token, "missing-run")
            .unwrap_err(),
        RegistryError::RunNotFound
    );
    let run = registry
        .register_launched(registration(RegistrationKind::Launched))
        .unwrap();
    registry.bind_launched_event_token(&token, &run.id).unwrap();
    registry.authenticate_launched_event_token(&token).unwrap();
}

#[test]
fn terminal_wide_revoke_and_exit_cover_every_matching_launched_run() {
    let root = TempDirectory::new("ptrack-agent-registry-terminal-wide");
    let now = Arc::new(AtomicI64::new(100));
    let registry = Registry::new(config(root.path(), now));
    let first = registry
        .register_launched(registration(RegistrationKind::Launched))
        .unwrap();
    let second = registry
        .register_launched(registration(RegistrationKind::Launched))
        .unwrap();
    let first_token = registry.issue_launched_event_token().unwrap();
    let second_token = registry.issue_launched_event_token().unwrap();
    registry
        .bind_launched_event_token(&first_token, &first.id)
        .unwrap();
    registry
        .bind_launched_event_token(&second_token, &second.id)
        .unwrap();
    assert!(registry.revoke_launched_event_token_for_terminal("terminal-1"));
    for token in [&first_token, &second_token] {
        assert_eq!(
            registry
                .authenticate_launched_event_token(token)
                .unwrap_err(),
            RegistryError::InvalidEventToken
        );
    }
    assert!(registry.record_terminal_exit("terminal-1", 17, "private output"));
    for id in [&first.id, &second.id] {
        let run = registry.run(id).unwrap();
        assert_eq!(run.state, RunState::Exited);
        assert_eq!(run.exit.unwrap().result, "failed");
    }
    assert!(registry.record_terminal_exit("terminal-1", 0, "done"));
    assert!(!registry.revoke_launched_event_token_for_terminal("terminal-1"));
}

#[test]
fn events_are_host_stamped_ordered_retained_and_move_activity_only_forward() {
    let root = TempDirectory::new("ptrack-agent-registry-events");
    let now = Arc::new(AtomicI64::new(100));
    let mut cfg = config(root.path(), Arc::clone(&now));
    cfg.event_policy = Some(EventPrivacyPolicy {
        retain_last: 1,
        ..default_event_privacy_policy()
    });
    let registry = Registry::new(cfg);
    let lease = registry
        .register_external(registration(RegistrationKind::External))
        .unwrap();
    now.store(101, Ordering::SeqCst);
    let first = registry
        .record_event(
            &lease.run.id,
            &lease.lease_token,
            observation("source-1", 1),
        )
        .unwrap();
    assert_eq!(first.run_id, lease.run.id);
    assert_eq!(first.provider, "codex");
    assert_eq!(first.host_sequence, 1);
    assert_eq!(first.lifecycle_revision, 1);
    assert_eq!(first.correlation.project_root, lease.run.project_root);
    assert_eq!(
        registry
            .record_event(
                &lease.run.id,
                &lease.lease_token,
                observation("source-2", 1)
            )
            .unwrap_err(),
        RegistryError::EventOrder
    );
    now.store(99, Ordering::SeqCst);
    let second = registry
        .record_event(
            &lease.run.id,
            &lease.lease_token,
            observation("source-2", 2),
        )
        .unwrap();
    assert_eq!(second.host_sequence, 2);
    assert_eq!(
        registry.run(&lease.run.id).unwrap().last_activity_at,
        first.observed_at
    );
    now.store(102, Ordering::SeqCst);
    let third = registry
        .record_event(
            &lease.run.id,
            &lease.lease_token,
            observation("source-1", 3),
        )
        .unwrap();
    assert_eq!(third.host_sequence, 3);
    let (events, total) = registry.event_snapshot(&lease.run.id, 0).unwrap();
    assert_eq!(total, 1);
    assert_eq!(events[0].id, third.id);
}

#[test]
fn event_authority_is_rechecked_after_normalization_across_stale_epoch() {
    let root = TempDirectory::new("ptrack-agent-registry-event-race");
    let now = Arc::new(AtomicI64::new(100));
    let call = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(Barrier::new(2));
    let resume = Arc::new(Barrier::new(2));
    let mut cfg = config(root.path(), Arc::clone(&now));
    cfg.random = Some(Arc::new({
        let call = Arc::clone(&call);
        let entered = Arc::clone(&entered);
        let resume = Arc::clone(&resume);
        move |bytes| {
            let current = call.fetch_add(1, Ordering::SeqCst);
            bytes.fill(u8::try_from(current).unwrap());
            if current == 2 {
                entered.wait();
                resume.wait();
            }
            Ok(())
        }
    }));
    let registry = Registry::new(cfg);
    let lease = registry
        .register_external(registration(RegistrationKind::External))
        .unwrap();
    std::thread::scope(|scope| {
        let writer = scope.spawn(|| {
            registry.record_event(
                &lease.run.id,
                &lease.lease_token,
                observation("source-1", 1),
            )
        });
        entered.wait();
        now.store(131, Ordering::SeqCst);
        registry.sweep_expired();
        resume.wait();
        assert_eq!(
            writer.join().unwrap().unwrap_err(),
            RegistryError::InvalidLease
        );
    });
    registry
        .heartbeat(&lease.run.id, &lease.lease_token)
        .unwrap();
    now.store(132, Ordering::SeqCst);
    let event = registry
        .record_event(
            &lease.run.id,
            &lease.lease_token,
            observation("source-1", 1),
        )
        .unwrap();
    assert_eq!(event.lifecycle_revision, 3);
}

#[test]
fn launched_token_revocation_between_authentication_and_commit_fails_closed() {
    let root = TempDirectory::new("ptrack-agent-registry-launched-event-race");
    let now = Arc::new(AtomicI64::new(100));
    let call = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(Barrier::new(2));
    let resume = Arc::new(Barrier::new(2));
    let mut cfg = config(root.path(), now);
    cfg.random = Some(Arc::new({
        let call = Arc::clone(&call);
        let entered = Arc::clone(&entered);
        let resume = Arc::clone(&resume);
        move |bytes| {
            let current = call.fetch_add(1, Ordering::SeqCst);
            bytes.fill(u8::try_from(current).unwrap());
            if current == 2 {
                entered.wait();
                resume.wait();
            }
            Ok(())
        }
    }));
    let registry = Registry::new(cfg);
    let run = registry
        .register_launched(registration(RegistrationKind::Launched))
        .unwrap();
    let token = registry.issue_launched_event_token().unwrap();
    registry.bind_launched_event_token(&token, &run.id).unwrap();
    let provider_event = ProviderEvent {
        model_version: PROVIDER_EVENT_MODEL_VERSION,
        id: "source-1".to_owned(),
        sequence: 1,
        event_type: "pretooluse".to_owned(),
        ..ProviderEvent::default()
    };
    std::thread::scope(|scope| {
        let writer =
            scope.spawn(|| registry.record_launched_provider_event(&token, provider_event));
        entered.wait();
        registry.revoke_launched_event_token(&token);
        resume.wait();
        assert_eq!(
            writer.join().unwrap().unwrap_err(),
            RegistryError::InvalidEventToken
        );
    });
    assert_eq!(registry.event_snapshot(&run.id, 0).unwrap().1, 0);
}

#[test]
fn exact_snapshot_callback_excludes_lifecycle_writer_until_return() {
    let root = TempDirectory::new("ptrack-agent-registry-exact-lock");
    let now = Arc::new(AtomicI64::new(100));
    let registry = Registry::new(config(root.path(), now));
    let lease = registry
        .register_external(registration(RegistrationKind::External))
        .unwrap();
    let barrier = Arc::new(Barrier::new(2));
    registry.install_heartbeat_barrier(Arc::clone(&barrier));
    let (sent, received) = mpsc::channel();
    std::thread::scope(|scope| {
        let writer = scope.spawn(|| {
            let result = registry.heartbeat(&lease.run.id, &lease.lease_token);
            sent.send(result).unwrap();
        });
        registry
            .with_exact_runtime_snapshot(1, |_| {
                barrier.wait();
                assert_eq!(received.try_recv(), Err(mpsc::TryRecvError::Empty));
                Ok(())
            })
            .unwrap();
        writer.join().unwrap();
    });
    received.recv().unwrap().unwrap();
}

#[test]
fn terminal_lifecycle_and_linked_association_fences_are_exact() {
    let root = TempDirectory::new("ptrack-agent-registry-linked");
    let now = Arc::new(AtomicI64::new(100));
    let registry = Registry::new(config(root.path(), Arc::clone(&now)));
    let catalog = Catalog;
    let host = AssociationHost::new(root.path(), 4, Some(&catalog)).unwrap();
    let pointer = AssociationPointer {
        version: 1,
        plan_id: 7,
        task_id: 9,
    };
    let run = registry
        .register_linked_launched(
            registration(RegistrationKind::Launched),
            Some(&host),
            pointer,
        )
        .unwrap();
    assert!(registry.is_linked_launch_run(&run.id));
    assert!(registry.has_linked_terminal("terminal-1"));
    assert_eq!(
        registry
            .associate(&run.id, Some(&host), pointer)
            .unwrap_err(),
        RegistryError::LinkedAssociation
    );
    let previous = run.association.clone().unwrap();
    let next = host.bind(&run.id, pointer, Some(&previous)).unwrap();
    let mut terminal_next = next.clone();
    terminal_next.live_id = "terminal-owned-id".to_owned();
    let change = registry
        .prepare_linked_terminal_association_change(
            "terminal-1",
            Some(&previous),
            &terminal_next,
            Some(&host),
            pointer,
        )
        .unwrap()
        .unwrap();
    registry.commit_linked_association_change(&change).unwrap();
    registry
        .rollback_linked_association_change(&change)
        .unwrap();

    now.store(105, Ordering::SeqCst);
    assert!(registry.record_terminal_activity("terminal-1"));
    assert_eq!(
        registry.run(&run.id).unwrap().last_activity_at,
        Timestamp::from_unix_seconds(105)
    );
    assert!(registry.record_terminal_exit("terminal-1", 0, "success"));
    assert_eq!(
        registry.run(&run.id).unwrap().exit.unwrap().result,
        "success"
    );
    assert!(!registry.rollback_linked_launched(&run.id, "other-terminal"));
    assert!(registry.rollback_linked_launched(&run.id, "terminal-1"));
}

#[test]
fn shutdown_closes_admission_and_pending_token_waiters() {
    let root = TempDirectory::new("ptrack-agent-registry-shutdown");
    let now = Arc::new(AtomicI64::new(100));
    let registry = Registry::new(config(root.path(), now));
    let token = registry.issue_launched_event_token().unwrap();
    registry.shutdown_timeout(Duration::from_secs(1)).unwrap();
    registry.shutdown_timeout(Duration::ZERO).unwrap();
    assert_eq!(
        registry.issue_launched_event_token().unwrap_err(),
        RegistryError::Closed
    );
    assert_eq!(
        registry
            .await_launched_event_token(&token, Duration::from_millis(1))
            .unwrap_err(),
        RegistryError::InvalidEventToken
    );
}

#[test]
fn shutdown_wakes_an_already_blocked_waiter_and_is_concurrently_idempotent() {
    let root = TempDirectory::new("ptrack-agent-registry-shutdown-race");
    let now = Arc::new(AtomicI64::new(100));
    let registry = Registry::new(config(root.path(), now));
    let token = registry.issue_launched_event_token().unwrap();
    let waiting = Arc::new(Barrier::new(2));
    registry.install_wait_barrier(Arc::clone(&waiting));
    std::thread::scope(|scope| {
        let waiter =
            scope.spawn(|| registry.await_launched_event_token(&token, Duration::from_secs(30)));
        waiting.wait();
        registry.shutdown_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(
            waiter.join().unwrap().unwrap_err(),
            RegistryError::InvalidEventToken
        );
    });

    let concurrent_registry = Registry::new(config(root.path(), Arc::new(AtomicI64::new(200))));
    let concurrent = Arc::new(Barrier::new(5));
    std::thread::scope(|scope| {
        let mut shutdowns = Vec::new();
        for _ in 0..4 {
            let concurrent = Arc::clone(&concurrent);
            let registry_ref = &concurrent_registry;
            shutdowns.push(scope.spawn(move || {
                concurrent.wait();
                registry_ref.shutdown().unwrap();
            }));
        }
        concurrent.wait();
        for shutdown in shutdowns {
            shutdown.join().unwrap();
        }
    });
}
