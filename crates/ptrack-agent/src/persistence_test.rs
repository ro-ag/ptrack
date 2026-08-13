use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, AtomicU8, Ordering};
use std::sync::{Arc, Barrier};

use super::*;
use crate::persistence::read_history;
use crate::test_support::TempDirectory;

fn persisted_config(root: &Path, state_path: PathBuf, seconds: Arc<AtomicI64>) -> RegistryConfig {
    let random = Arc::new(AtomicU8::new(0));
    RegistryConfig {
        project_root: root.to_path_buf(),
        state_path,
        now: Some(Arc::new(move || {
            Timestamp::from_unix_seconds(seconds.load(Ordering::SeqCst))
        })),
        random: Some(Arc::new(move |bytes| {
            bytes.fill(random.fetch_add(1, Ordering::SeqCst));
            Ok(())
        })),
        ..RegistryConfig::default()
    }
}

fn external_registration() -> Registration {
    Registration {
        profile: "wrapper".to_owned(),
        provider: "codex".to_owned(),
        ..Registration::default()
    }
}

fn launched_registration() -> Registration {
    Registration {
        profile: "agent-codex".to_owned(),
        provider: "codex".to_owned(),
        pid: 42,
        terminal_id: "terminal-1".to_owned(),
        ..Registration::default()
    }
}

#[test]
fn runtime_layout_hash_is_exact_and_creates_nothing() {
    let home = TempDirectory::new("ptrack-agent-runtime-home");
    let path = runtime_dir(home.path(), "/project/./nested/..").unwrap();
    assert_eq!(
        path,
        home.path()
            .join("runtime")
            .join("ea0135bca5e3bd815f5b7b8f8c83d86f584697bc29e0cc3b30937153abef2844")
    );
    assert!(!path.exists());
    assert_eq!(
        run_history_path(home.path(), "/project").unwrap(),
        path.join("agent-runs.json")
    );
}

#[test]
fn v3_history_json_shape_and_trailing_newline_are_golden() {
    let root = TempDirectory::new("ptrack-agent-history-golden-root");
    let history = TempDirectory::new("ptrack-agent-history-golden-file");
    let state_path = history.path().join("agent-runs.json");
    let now = Arc::new(AtomicI64::new(100));
    let registry = Registry::new(persisted_config(root.path(), state_path.clone(), now));
    let lease = registry.register_external(external_registration()).unwrap();
    assert_eq!(lease.run.id, "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
    assert_eq!(
        lease.lease_token,
        "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE"
    );
    let root_json = serde_json::to_string(&lease.run.project_root).unwrap();
    let id_json = serde_json::to_string(&lease.run.id).unwrap();
    let token_json = serde_json::to_string(&lease.lease_token).unwrap();
    let expected = format!(
        concat!(
            r#"{{"version":3,"savedAt":"1970-01-01T00:01:40Z","runs":[{{"run":{{"id":{id},"profile":"wrapper","provider":"codex","pid":0,"processState":"unknown","leaseState":"active","projectRoot":{root},"terminalId":"","cwd":{root},"startedAt":"1970-01-01T00:01:40Z","lastActivityAt":"1970-01-01T00:01:40Z","lastHeartbeatAt":"1970-01-01T00:01:40Z","state":"running","registrationKind":"external"}},"leaseToken":{token}}}]}}"#,
            "\n"
        ),
        id = id_json,
        root = root_json,
        token = token_json,
    );
    assert_eq!(fs::read_to_string(&state_path).unwrap(), expected);
}

#[test]
fn restart_restores_external_lease_and_marks_only_interrupted_launch_stale() {
    let root = TempDirectory::new("ptrack-agent-history-restart-root");
    let history = TempDirectory::new("ptrack-agent-history-restart-file");
    let state_path = history.path().join("agent-runs.json");
    let now = Arc::new(AtomicI64::new(100));
    let first = Registry::new(persisted_config(
        root.path(),
        state_path.clone(),
        Arc::clone(&now),
    ));
    let launched = first.register_launched(launched_registration()).unwrap();
    let event_token = first.issue_launched_event_token().unwrap();
    first
        .bind_launched_event_token(&event_token, &launched.id)
        .unwrap();
    let external = first.register_external(external_registration()).unwrap();
    first.shutdown().unwrap();
    let contents = fs::read_to_string(&state_path).unwrap();
    assert!(!contents.contains(&event_token));
    assert!(!contents.contains("eventToken"));
    assert!(!contents.contains("association"));

    let second = Registry::new(persisted_config(root.path(), state_path, Arc::clone(&now)));
    let restored_launched = second.run(&launched.id).unwrap();
    assert_eq!(restored_launched.state, RunState::Stale);
    assert_eq!(restored_launched.process_state, ProcessState::Unknown);
    let restored_external = second.run(&external.run.id).unwrap();
    assert_eq!(restored_external.state, RunState::Running);
    assert_eq!(restored_external.lease_state, LeaseState::Active);
    second
        .heartbeat(&external.run.id, &external.lease_token)
        .unwrap();
    assert_eq!(
        second
            .authenticate_launched_event_token(&event_token)
            .unwrap_err(),
        RegistryError::InvalidEventToken
    );
}

#[test]
fn restored_sibling_launch_is_revalidated_detached_and_loses_linked_provenance() {
    let root = TempDirectory::new("ptrack-agent-history-sibling-root");
    let sibling = TempDirectory::new("ptrack-agent-history-sibling-cwd");
    let history = TempDirectory::new("ptrack-agent-history-sibling-file");
    let state_path = history.path().join("agent-runs.json");
    let canonical_sibling = fs::canonicalize(sibling.path()).unwrap();
    let now = Arc::new(AtomicI64::new(100));
    let mut first_config = persisted_config(root.path(), state_path.clone(), Arc::clone(&now));
    let expected = canonical_sibling.clone();
    first_config.additional_cwd_validator = Some(Arc::new(move |path| path == expected));
    let first = Registry::new(first_config);
    let host = AssociationHost::new(root.path(), 1, None).unwrap();
    let mut registration = launched_registration();
    registration.cwd = canonical_sibling.to_string_lossy().into_owned();
    let run = first
        .register_linked_launched(
            registration,
            Some(&host),
            AssociationPointer {
                version: ASSOCIATION_VERSION_V1,
                ..AssociationPointer::default()
            },
        )
        .unwrap();
    assert!(first.is_linked_launch_run(&run.id));
    first.shutdown().unwrap();

    let validations = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut second_config = persisted_config(root.path(), state_path, Arc::clone(&now));
    let expected = canonical_sibling.clone();
    let calls = Arc::clone(&validations);
    second_config.additional_cwd_validator = Some(Arc::new(move |path| {
        calls.fetch_add(1, Ordering::SeqCst);
        path == expected
    }));
    let second = Registry::new(second_config);
    let restored = second.run(&run.id).unwrap();
    assert_eq!(restored.state, RunState::Stale);
    assert_eq!(restored.cwd, canonical_sibling.to_string_lossy());
    assert!(restored.association.is_none());
    assert!(!second.is_linked_launch_run(&run.id));
    assert_eq!(validations.load(Ordering::SeqCst), 1);
}

#[test]
fn restored_history_rewrites_on_sweep_but_never_on_heartbeat() {
    let root = TempDirectory::new("ptrack-agent-history-dirty-root");
    let history = TempDirectory::new("ptrack-agent-history-dirty-file");
    let state_path = history.path().join("agent-runs.json");
    let now = Arc::new(AtomicI64::new(100));
    let first = Registry::new(persisted_config(
        root.path(),
        state_path.clone(),
        Arc::clone(&now),
    ));
    let lease = first.register_external(external_registration()).unwrap();
    first.shutdown().unwrap();
    let before = fs::read(&state_path).unwrap();

    now.store(200, Ordering::SeqCst);
    let second = Registry::new(persisted_config(
        root.path(),
        state_path.clone(),
        Arc::clone(&now),
    ));
    second.heartbeat(&lease.run.id, &lease.lease_token).unwrap();
    assert_eq!(fs::read(&state_path).unwrap(), before);
    second.sweep_expired();
    let after = fs::read_to_string(&state_path).unwrap();
    assert_ne!(after.as_bytes(), before);
    assert!(after.contains(r#""savedAt":"1970-01-01T00:03:20Z""#));
}

#[test]
fn corrupt_is_advisory_future_is_never_clobbered_and_v1_migrates_detached() {
    let root = TempDirectory::new("ptrack-agent-history-migrate-root");
    let history = TempDirectory::new("ptrack-agent-history-migrate-file");
    let state_path = history.path().join("agent-runs.json");
    fs::write(&state_path, "{not json").unwrap();
    let now = Arc::new(AtomicI64::new(100));
    let corrupt = Registry::new(persisted_config(
        root.path(),
        state_path.clone(),
        Arc::clone(&now),
    ));
    assert!(corrupt.snapshot(10).is_empty());
    corrupt.register_external(external_registration()).unwrap();
    assert!(
        fs::read_to_string(&state_path)
            .unwrap()
            .starts_with("{\"version\":3,")
    );
    corrupt.shutdown().unwrap();

    let future =
        br#"{"version":999,"savedAt":"2026-08-10T00:00:00Z","runs":[],"futureCanary":"PRESERVE"}"#
            .to_vec();
    fs::write(&state_path, &future).unwrap();
    let future_registry = Registry::new(persisted_config(
        root.path(),
        state_path.clone(),
        Arc::clone(&now),
    ));
    future_registry
        .register_external(external_registration())
        .unwrap();
    future_registry.shutdown().unwrap();
    assert_eq!(fs::read(&state_path).unwrap(), future);
    assert_eq!(
        PersistenceError::FutureVersion { found: 999 }.to_string(),
        "AgentRun history is newer than supported: version 999 exceeds 3"
    );

    let project_root = fs::canonicalize(root.path())
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let root_json = serde_json::to_string(&project_root).unwrap();
    let legacy = format!(
        r#"{{"version":1,"savedAt":"1970-01-01T00:01:40Z","runs":[{{"run":{{"id":"legacy-run","profile":"wrapper","provider":"codex","projectRoot":{root_json},"planId":2,"taskId":9,"cwd":{root_json},"state":"running","processState":"unknown","leaseState":"active","registrationKind":"external","lastActivityAt":"1970-01-01T00:01:40Z","lastHeartbeatAt":"1970-01-01T00:01:40Z"}},"leaseToken":"legacy-lease"}}]}}"#,
    );
    fs::write(&state_path, legacy).unwrap();
    let migrated = Registry::new(persisted_config(
        root.path(),
        state_path.clone(),
        Arc::clone(&now),
    ));
    assert!(migrated.run("legacy-run").unwrap().association.is_none());
    migrated.heartbeat("legacy-run", "legacy-lease").unwrap();
    migrated.sweep_expired();
    let migrated_json = fs::read_to_string(&state_path).unwrap();
    assert!(migrated_json.starts_with("{\"version\":3,"));
    assert!(!migrated_json.contains("planId"));
    assert!(!migrated_json.contains("taskId"));
    assert!(!migrated_json.contains("association"));
}

#[test]
fn future_version_preflight_preserves_unknown_changed_shapes() {
    let root = TempDirectory::new("ptrack-agent-history-future-shape-root");
    let history = TempDirectory::new("ptrack-agent-history-future-shape-file");
    let state_path = history.path().join("agent-runs.json");
    let future = br#"{"version":4294967296,"savedAt":{"future":"timestamp-shape"},"runs":[{"run":{"registrationKind":"future-kind","state":{"future":true}}}],"futureCanary":"PRESERVE"}"#.to_vec();
    fs::write(&state_path, &future).unwrap();
    assert_eq!(
        read_history(&state_path).unwrap_err(),
        PersistenceError::FutureVersion {
            found: 4_294_967_296
        }
    );
    let registry = Registry::new(persisted_config(
        root.path(),
        state_path.clone(),
        Arc::new(AtomicI64::new(100)),
    ));
    registry.register_external(external_registration()).unwrap();
    registry.sweep_expired();
    registry.shutdown().unwrap();
    assert_eq!(fs::read(&state_path).unwrap(), future);
}

#[test]
fn late_future_replacement_disables_mutation_sweep_and_shutdown_writes() {
    let root = TempDirectory::new("ptrack-agent-history-late-future-root");
    let history = TempDirectory::new("ptrack-agent-history-late-future-file");
    let state_path = history.path().join("agent-runs.json");
    let now = Arc::new(AtomicI64::new(100));
    let registry = Registry::new(persisted_config(
        root.path(),
        state_path.clone(),
        Arc::clone(&now),
    ));
    let lease = registry.register_external(external_registration()).unwrap();
    let future = br#"{"version":4294967296,"savedAt":"future","runs":"changed","futureCanary":"LATE_PRESERVE"}"#.to_vec();
    fs::write(&state_path, &future).unwrap();
    registry
        .exit_external(&lease.run.id, &lease.lease_token, 9, "private")
        .unwrap();
    now.store(200, Ordering::SeqCst);
    registry.sweep_expired();
    registry.shutdown().unwrap();
    assert_eq!(fs::read(&state_path).unwrap(), future);
}

#[test]
fn restore_strips_launched_lease_tokens_and_keeps_newest_duplicate_id() {
    let root = TempDirectory::new("ptrack-agent-history-duplicate-root");
    let history = TempDirectory::new("ptrack-agent-history-duplicate-file");
    let state_path = history.path().join("agent-runs.json");
    let canonical = fs::canonicalize(root.path())
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let run = |profile: &str, activity: i64| {
        serde_json::json!({
            "run": {
                "id": "duplicate",
                "profile": profile,
                "provider": "codex",
                "pid": 42,
                "processState": "running",
                "leaseState": "none",
                "projectRoot": canonical,
                "terminalId": "terminal-1",
                "cwd": canonical,
                "startedAt": Timestamp::from_unix_seconds(50),
                "lastActivityAt": Timestamp::from_unix_seconds(activity),
                "state": "running",
                "registrationKind": "launched"
            },
            "leaseToken": "LAUNCHED_LEASE_SECRET_CANARY"
        })
    };
    fs::write(
        &state_path,
        serde_json::to_vec(&serde_json::json!({
            "version": 3,
            "savedAt": Timestamp::from_unix_seconds(300),
            "runs": [run("older", 100), run("newest", 200)]
        }))
        .unwrap(),
    )
    .unwrap();
    let registry = Registry::new(persisted_config(
        root.path(),
        state_path.clone(),
        Arc::new(AtomicI64::new(300)),
    ));
    assert_eq!(registry.run("duplicate").unwrap().profile, "newest");
    registry.sweep_expired();
    let rewritten = fs::read_to_string(&state_path).unwrap();
    assert!(!rewritten.contains("LAUNCHED_LEASE_SECRET_CANARY"));
    assert_eq!(rewritten.matches(r#""id":"duplicate""#).count(), 1);
}

#[test]
fn restore_is_bounded_to_1024_most_recent_records() {
    let root = TempDirectory::new("ptrack-agent-history-bounded-root");
    let history = TempDirectory::new("ptrack-agent-history-bounded-file");
    let state_path = history.path().join("agent-runs.json");
    let canonical = fs::canonicalize(root.path())
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let mut runs = Vec::new();
    for index in 0..1_025 {
        runs.push(serde_json::json!({
            "run": {
                "id": format!("run-{index}"),
                "profile": "p",
                "provider": "codex",
                "projectRoot": canonical,
                "cwd": canonical,
                "lastActivityAt": Timestamp::from_unix_seconds(index),
                "state": "exited",
                "registrationKind": "external"
            }
        }));
    }
    fs::write(
        &state_path,
        serde_json::to_vec(&serde_json::json!({
            "version": 3,
            "savedAt": Timestamp::from_unix_seconds(2_000),
            "runs": runs
        }))
        .unwrap(),
    )
    .unwrap();
    let registry = Registry::new(persisted_config(
        root.path(),
        state_path,
        Arc::new(AtomicI64::new(2_000)),
    ));
    assert_eq!(registry.snapshot_bounded(64).1, 1_024);
    assert_eq!(
        registry.run("run-0").unwrap_err(),
        RegistryError::RunNotFound
    );
    assert_eq!(registry.run("run-1024").unwrap().id, "run-1024");
}

#[test]
fn retained_events_are_revalidated_and_rebuild_host_epochs_and_ordering() {
    let root = TempDirectory::new("ptrack-agent-history-events-root");
    let history = TempDirectory::new("ptrack-agent-history-events-file");
    let state_path = history.path().join("agent-runs.json");
    let now = Arc::new(AtomicI64::new(100));
    let first = Registry::new(persisted_config(
        root.path(),
        state_path.clone(),
        Arc::clone(&now),
    ));
    let lease = first.register_external(external_registration()).unwrap();
    let recorded = first
        .record_event(
            &lease.run.id,
            &lease.lease_token,
            EventObservation {
                model_version: EVENT_MODEL_VERSION,
                source_id: "source-9".to_owned(),
                source_sequence: 9,
                kind: EventKind::Tool,
                phase: EventPhase::Progress,
                ..EventObservation::default()
            },
        )
        .unwrap();
    first.shutdown().unwrap();
    let second = Registry::new(persisted_config(root.path(), state_path, Arc::clone(&now)));
    let (events, total) = second.event_snapshot(&lease.run.id, 10).unwrap();
    assert_eq!(total, 1);
    assert_eq!(events[0].id, recorded.id);
    assert_eq!(events[0].source_sequence, 9);
    second
        .with_exact_runtime_snapshot(1, |runs| {
            assert_eq!(runs[0].lifecycle_revision, 2);
            Ok(())
        })
        .unwrap();
    assert_eq!(
        second
            .record_event(
                &lease.run.id,
                &lease.lease_token,
                EventObservation {
                    model_version: EVENT_MODEL_VERSION,
                    source_id: "source-10".to_owned(),
                    source_sequence: 9,
                    kind: EventKind::Tool,
                    phase: EventPhase::Progress,
                    ..EventObservation::default()
                },
            )
            .unwrap_err(),
        RegistryError::EventOrder
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn runtime_json_publish_compare_remove_liveness_and_concurrency_are_private() {
    let home = TempDirectory::new("ptrack-agent-descriptor-home");
    let root = TempDirectory::new("ptrack-agent-descriptor-root");
    let descriptor = IntegrationDescriptor {
        project_root: fs::canonicalize(root.path())
            .unwrap()
            .to_string_lossy()
            .into_owned(),
        url: "http://127.0.0.1:1234".to_owned(),
        generation: 7,
        registration_token: "DESCRIPTOR_SECRET_CANARY".to_owned(),
        pid: i32::try_from(std::process::id()).unwrap(),
    };
    let debug = format!("{descriptor:?}");
    assert!(debug.contains("[redacted]"));
    assert!(!debug.contains("DESCRIPTOR_SECRET_CANARY"));
    let path =
        publish_runtime_json(home.path(), root.path(), "agent-registry.json", &descriptor).unwrap();
    assert_eq!(
        read_integration_descriptor(home.path(), root.path()).unwrap(),
        descriptor
    );
    assert!(fs::read_to_string(&path).unwrap().ends_with('\n'));
    let mut stale = descriptor.clone();
    stale.pid = 0;
    publish_runtime_json(home.path(), root.path(), "agent-registry.json", &stale).unwrap();
    assert_eq!(
        read_integration_descriptor(home.path(), root.path()).unwrap_err(),
        PersistenceError::DescriptorStale { pid: 0 }
    );
    publish_runtime_json(home.path(), root.path(), "agent-registry.json", &descriptor).unwrap();

    let barrier = Arc::new(Barrier::new(3));
    std::thread::scope(|scope| {
        for generation in [8_u64, 9] {
            let barrier = Arc::clone(&barrier);
            let mut replacement = descriptor.clone();
            replacement.generation = generation;
            let home_path = home.path().to_path_buf();
            let root_path = root.path().to_path_buf();
            scope.spawn(move || {
                barrier.wait();
                publish_runtime_json(home_path, root_path, "agent-registry.json", &replacement)
                    .unwrap();
            });
        }
        barrier.wait();
    });
    let final_descriptor: IntegrationDescriptor =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert!([8, 9].contains(&final_descriptor.generation));
    remove_runtime_json_if_equal(home.path(), root.path(), "agent-registry.json", &descriptor)
        .unwrap();
    assert!(path.exists());
    remove_runtime_json_if_equal(
        home.path(),
        root.path(),
        "agent-registry.json",
        &final_descriptor,
    )
    .unwrap();
    assert!(!path.exists());

    let numeric_path = publish_runtime_json(
        home.path(),
        root.path(),
        "numeric.json",
        &serde_json::json!({"value": 1}),
    )
    .unwrap();
    remove_runtime_json_if_equal(
        home.path(),
        root.path(),
        "numeric.json",
        &serde_json::json!({"value": 1.0}),
    )
    .unwrap();
    assert!(!numeric_path.exists());
    let corrupt_path = publish_runtime_json(
        home.path(),
        root.path(),
        "corrupt.json",
        &serde_json::json!({"safe": true}),
    )
    .unwrap();
    fs::write(&corrupt_path, "{not json").unwrap();
    remove_runtime_json_if_equal(
        home.path(),
        root.path(),
        "corrupt.json",
        &serde_json::json!({"safe": true}),
    )
    .unwrap();
    assert!(corrupt_path.exists());
    for invalid in ["", ".", "../escape", "nested/file"] {
        assert_eq!(
            publish_runtime_json(home.path(), root.path(), invalid, &()).unwrap_err(),
            PersistenceError::InvalidDescriptorName
        );
    }
    assert_eq!(
        read_integration_descriptor(home.path(), root.path()).unwrap_err(),
        PersistenceError::DescriptorNotFound
    );
}

#[test]
fn shutdown_timeout_surfaces_final_history_save_failure() {
    let root = TempDirectory::new("ptrack-agent-history-error-root");
    let history = TempDirectory::new("ptrack-agent-history-error-file");
    let state_path = history
        .path()
        .join("blocked-parent")
        .join("agent-runs.json");
    fs::write(history.path().join("blocked-parent"), "not a directory").unwrap();
    let registry = Registry::new(persisted_config(
        root.path(),
        state_path,
        Arc::new(AtomicI64::new(100)),
    ));
    registry.register_external(external_registration()).unwrap();
    let error = registry.shutdown().unwrap_err();
    assert!(error.to_string().starts_with("write AgentRun history:"));
}

#[cfg(unix)]
#[test]
fn unix_runtime_permissions_and_nofollow_lock_are_enforced() {
    use std::os::unix::fs::{MetadataExt, symlink};

    let home = TempDirectory::new("ptrack-agent-private-home");
    let root = TempDirectory::new("ptrack-agent-private-root");
    let path = publish_runtime_json(
        home.path(),
        root.path(),
        "private.json",
        &serde_json::json!({"safe": true}),
    )
    .unwrap();
    let directory = path.parent().unwrap();
    assert_eq!(fs::metadata(directory).unwrap().mode() & 0o777, 0o700);
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
    assert_eq!(
        fs::metadata(directory.join(".agent-registry.lock"))
            .unwrap()
            .mode()
            & 0o777,
        0o600
    );

    fs::remove_file(directory.join(".agent-registry.lock")).unwrap();
    let canary = home.path().join("lock-canary");
    fs::write(&canary, "UNCHANGED").unwrap();
    symlink(&canary, directory.join(".agent-registry.lock")).unwrap();
    assert!(
        publish_runtime_json(
            home.path(),
            root.path(),
            "blocked.json",
            &serde_json::json!({"blocked": true}),
        )
        .is_err()
    );
    assert_eq!(fs::read_to_string(canary).unwrap(), "UNCHANGED");
}

#[cfg(unix)]
#[test]
fn pinned_runtime_directory_rejects_deterministic_parent_swap() {
    use crate::persistence::install_after_pin_hook;
    use std::os::unix::fs::PermissionsExt;

    let home = TempDirectory::new("ptrack-agent-pin-race-home");
    let root = TempDirectory::new("ptrack-agent-pin-race-root");
    let directory = runtime_dir(home.path(), root.path()).unwrap();
    fs::create_dir_all(&directory).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
    let displaced = directory.with_extension("displaced");
    let hook_directory = directory.clone();
    let hook_displaced = displaced.clone();
    install_after_pin_hook(move || {
        fs::rename(&hook_directory, &hook_displaced).unwrap();
        fs::create_dir(&hook_directory).unwrap();
        fs::set_permissions(&hook_directory, fs::Permissions::from_mode(0o700)).unwrap();
    });
    assert!(
        publish_runtime_json(
            home.path(),
            root.path(),
            "race.json",
            &serde_json::json!({"safe": true}),
        )
        .is_err()
    );
    assert!(!directory.join("race.json").exists());
    assert!(!displaced.join("race.json").exists());
}

#[cfg(unix)]
#[test]
fn pinned_runtime_directory_cleans_owned_temp_after_pre_rename_swap() {
    use crate::persistence::install_before_rename_hook;
    use std::os::unix::fs::PermissionsExt;

    let home = TempDirectory::new("ptrack-agent-pre-rename-race-home");
    let root = TempDirectory::new("ptrack-agent-pre-rename-race-root");
    let directory = runtime_dir(home.path(), root.path()).unwrap();
    fs::create_dir_all(&directory).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
    let displaced = directory.with_extension("displaced");
    let hook_directory = directory.clone();
    let hook_displaced = displaced.clone();
    install_before_rename_hook(move || {
        fs::rename(&hook_directory, &hook_displaced).unwrap();
        fs::create_dir(&hook_directory).unwrap();
        fs::set_permissions(&hook_directory, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(
            hook_directory.join("race.json"),
            "UNRELATED_REPLACEMENT_CANARY",
        )
        .unwrap();
    });
    assert!(
        publish_runtime_json(
            home.path(),
            root.path(),
            "race.json",
            &serde_json::json!({"safe": true}),
        )
        .is_err()
    );
    assert_eq!(
        fs::read_to_string(directory.join("race.json")).unwrap(),
        "UNRELATED_REPLACEMENT_CANARY"
    );
    assert!(!displaced.join("race.json").exists());
    assert!(fs::read_dir(&displaced).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".race.json-")
    }));
}

#[cfg(unix)]
#[test]
fn pinned_runtime_directory_cleans_owned_final_after_post_rename_swap() {
    use crate::persistence::install_after_rename_hook;
    use std::os::unix::fs::PermissionsExt;

    let home = TempDirectory::new("ptrack-agent-post-rename-race-home");
    let root = TempDirectory::new("ptrack-agent-post-rename-race-root");
    let directory = runtime_dir(home.path(), root.path()).unwrap();
    fs::create_dir_all(&directory).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
    let displaced = directory.with_extension("displaced");
    let hook_directory = directory.clone();
    let hook_displaced = displaced.clone();
    install_after_rename_hook(move || {
        fs::rename(&hook_directory, &hook_displaced).unwrap();
        fs::create_dir(&hook_directory).unwrap();
        fs::set_permissions(&hook_directory, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(
            hook_directory.join("race.json"),
            "UNRELATED_REPLACEMENT_CANARY",
        )
        .unwrap();
    });
    assert!(
        publish_runtime_json(
            home.path(),
            root.path(),
            "race.json",
            &serde_json::json!({"safe": true}),
        )
        .is_err()
    );
    assert_eq!(
        fs::read_to_string(directory.join("race.json")).unwrap(),
        "UNRELATED_REPLACEMENT_CANARY"
    );
    assert!(!displaced.join("race.json").exists());
    assert!(fs::read_dir(&displaced).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".race.json-")
    }));
}
