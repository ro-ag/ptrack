use std::net::TcpListener;
use std::sync::{Arc, Barrier};
use std::time::Duration;

use ptrack_capability_policy::{confirm_approval, normalize};
use ptrack_store::ProjectStore;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::broker::{
    Broker, BrokerConfig, TOOL_GIT, TOOL_HTTP_REQUEST, TOOL_SSH, ToolCall, tool_definitions,
};
use super::test_support::{TempDir, approved_http, store_at};

pub(super) fn assert_cap_037_through_044_broker_contract() {
    let definitions = tool_definitions();
    assert_eq!(definitions.len(), 3);
    assert_eq!(definitions[0].name, TOOL_HTTP_REQUEST);
    assert_eq!(definitions[1].name, TOOL_GIT);
    assert_eq!(definitions[2].name, TOOL_SSH);
    for tool in definitions {
        assert_eq!(tool.input_schema["additionalProperties"], false);
        assert_eq!(tool.annotations["destructiveHint"], true);
        assert_eq!(tool.annotations["openWorldHint"], true);
    }
}

#[test]
fn token_is_raw_32_bytes_hash_only_bind_once_and_generation_bound() {
    let temp = TempDir::new("broker-token");
    let (store, database, binding) = store_at(&temp);
    drop(store);
    let broker = Broker::new(BrokerConfig {
        project_root: temp.path().to_path_buf(),
        database,
        binding,
        writer_version: "test".to_owned(),
        generation: 1,
    })
    .unwrap();
    let token = broker.issue_session_token("agent-codex").unwrap();
    assert_eq!(token.len(), 43);
    assert!(broker.bind_session(&token, "session-one").is_ok());
    assert_eq!(
        broker
            .bind_session(&token, "session-two")
            .unwrap_err()
            .to_string(),
        "capability session token cannot be bound"
    );
    broker.revoke_session("session-one");
    assert_eq!(
        broker.authenticate_token(&token).unwrap_err().to_string(),
        "capability denied: capability session token is invalid or stale"
    );
}

#[tokio::test]
async fn strict_arguments_double_reload_and_revocation_deny_before_transport() {
    let temp = TempDir::new("broker-dispatch");
    let (store, database, binding) = store_at(&temp);
    let mut capability = approved_http("http://127.0.0.1:9/api");
    capability.id = 0;
    capability.revision = 0;
    capability.enabled = false;
    capability.approved_at = ptrack_core::Timestamp::Zero;
    capability.expires_at = ptrack_core::Timestamp::Zero;
    capability = store.add_capability(capability).unwrap();
    let proof =
        confirm_approval(&capability, normalize(&capability).unwrap().scope_digest).unwrap();
    capability = store.approve_capability(proof).unwrap();
    drop(store);
    let broker = Arc::new(
        Broker::new(BrokerConfig {
            project_root: temp.path().to_path_buf(),
            database,
            binding,
            writer_version: "test".to_owned(),
            generation: 1,
        })
        .unwrap(),
    );
    let token = broker.issue_session_token("agent-codex").unwrap();
    broker.bind_session(&token, "session").unwrap();
    let invalid = broker
        .call(
            &CancellationToken::new(),
            &token,
            ToolCall {
                name: TOOL_HTTP_REQUEST.to_owned(),
                arguments: json!({
                    "capability_id": capability.id,
                    "request": {"method": "GET", "url": "http://127.0.0.1:9/api", "unknown": true}
                }),
            },
        )
        .await
        .unwrap_err();
    assert!(invalid.to_string().starts_with("invalid tool arguments:"));
    broker.revoke_capability(capability.id);
    broker.shutdown();
    assert!(broker.issue_session_token("agent-codex").is_err());
}

#[test]
fn concurrency_dual_slots_revocation_shutdown_and_second_reload_are_atomic_and_recover() {
    let temp = TempDir::new("broker-races");
    let (store, database, binding) = store_at(&temp);
    let mut first = approved_http("http://127.0.0.1:9/api");
    first.id = 0;
    first.revision = 0;
    first.limits.max_concurrent = 1;
    first = store.add_capability(first).unwrap();
    let proof = confirm_approval(&first, normalize(&first).unwrap().scope_digest).unwrap();
    first = store.approve_capability(proof).unwrap();
    let mut second = approved_http("http://127.0.0.1:9/api");
    second.id = 0;
    second.revision = 0;
    second.limits.max_concurrent = 1;
    second = store.add_capability(second).unwrap();
    let proof = confirm_approval(&second, normalize(&second).unwrap().scope_digest).unwrap();
    second = store.approve_capability(proof).unwrap();
    drop(store);
    let broker = Broker::new(BrokerConfig {
        project_root: temp.path().to_path_buf(),
        database: database.clone(),
        binding: binding.clone(),
        writer_version: "test".to_owned(),
        generation: 1,
    })
    .unwrap();
    let token = broker.issue_session_token("agent-codex").unwrap();
    broker.bind_session(&token, "session").unwrap();

    let first_cancel = CancellationToken::new();
    let first_guard = broker.track(&[(first.id, 1)], &first_cancel).unwrap();
    let Err(error) = broker.track(&[(first.id, 1)], &CancellationToken::new()) else {
        panic!("occupied concurrency slot was accepted");
    };
    assert_eq!(
        error.to_string(),
        "capability denied: capability concurrency limit reached"
    );
    let second_guard = broker
        .track(&[(second.id, 1)], &CancellationToken::new())
        .unwrap();
    assert!(
        broker
            .track(&[(first.id, 1), (second.id, 1)], &CancellationToken::new())
            .is_err()
    );
    drop(second_guard);
    let second_recovered = broker
        .track(&[(second.id, 1)], &CancellationToken::new())
        .unwrap();
    drop(second_recovered);
    assert!(
        broker
            .track(&[(second.id, 1), (second.id, 1)], &CancellationToken::new())
            .is_err()
    );
    let duplicate_recovered = broker
        .track(&[(second.id, 1)], &CancellationToken::new())
        .unwrap();
    drop(duplicate_recovered);

    broker.revoke_capability(first.id);
    assert!(first_cancel.is_cancelled());
    drop(first_guard);
    let permit_recovered = broker
        .track(&[(first.id, 1)], &CancellationToken::new())
        .unwrap();
    drop(permit_recovered);

    let before = broker.load_capabilities(&[first.id]).unwrap().remove(0);
    assert!(before.enabled);
    let before_digest = before.scope_digest;
    let store = ProjectStore::open_existing(&database, &binding, "test").unwrap();
    let mut edit = before;
    edit.http
        .as_mut()
        .unwrap()
        .methods
        .push("DELETE".to_owned());
    edit = normalize(&edit).unwrap().capability;
    let edited = store.update_capability(edit).unwrap();
    assert!(!edited.enabled);
    drop(store);
    let after = broker.load_capabilities(&[first.id]).unwrap().remove(0);
    assert!(!after.enabled);
    assert_ne!(after.scope_digest, before_digest);

    let shutdown_cancel = CancellationToken::new();
    let _active = broker.track(&[(second.id, 1)], &shutdown_cancel).unwrap();
    broker.shutdown();
    assert!(shutdown_cancel.is_cancelled());
    assert!(broker.authenticate_token(&token).is_err());
    assert!(broker.issue_session_token("agent-codex").is_err());
}

#[test]
fn production_call_second_reload_observes_security_edit_before_transport() {
    let temp = TempDir::new("broker-production-reload");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let target = format!("http://{}/api", listener.local_addr().unwrap());
    let (store, database, binding) = store_at(&temp);
    let mut capability = approved_http(&target);
    capability.id = 0;
    capability.revision = 0;
    capability.enabled = false;
    capability.approved_at = ptrack_core::Timestamp::Zero;
    capability.expires_at = ptrack_core::Timestamp::Zero;
    capability = store.add_capability(capability).unwrap();
    let proof =
        confirm_approval(&capability, normalize(&capability).unwrap().scope_digest).unwrap();
    capability = store.approve_capability(proof).unwrap();
    drop(store);

    let broker = Arc::new(
        Broker::new(BrokerConfig {
            project_root: temp.path().to_path_buf(),
            database: database.clone(),
            binding: binding.clone(),
            writer_version: "test".to_owned(),
            generation: 1,
        })
        .unwrap(),
    );
    let token = broker.issue_session_token("agent-codex").unwrap();
    broker.bind_session(&token, "reload-session").unwrap();
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let worker_broker = Arc::clone(&broker);
    let worker_entered = Arc::clone(&entered);
    let worker_release = Arc::clone(&release);
    let worker_token = token.clone();
    let capability_id = capability.id;
    let worker = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(worker_broker.call_with_reload_barrier(
            &CancellationToken::new(),
            &worker_token,
            ToolCall {
                name: TOOL_HTTP_REQUEST.to_owned(),
                arguments: json!({
                    "capability_id": capability_id,
                    "request": {"method": "GET", "url": target}
                }),
            },
            &move || {
                worker_entered.wait();
                worker_release.wait();
            },
        ))
    });

    entered.wait();
    let store = ProjectStore::open_existing(&database, &binding, "test").unwrap();
    let mut edit = store.capability(capability.id).unwrap();
    edit.http
        .as_mut()
        .unwrap()
        .methods
        .push("DELETE".to_owned());
    edit = normalize(&edit).unwrap().capability;
    let edited = store.update_capability(edit).unwrap();
    assert!(!edited.enabled);
    drop(store);
    release.wait();

    let error = worker.join().unwrap().unwrap_err();
    assert!(error.to_string().contains("capability is disabled"));
    std::thread::sleep(Duration::from_millis(50));
    assert!(matches!(
        listener.accept(),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
    ));
}
