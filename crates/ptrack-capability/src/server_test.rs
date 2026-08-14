use std::fs;
use std::time::Duration;

use ptrack_agent::publish_runtime_json;
use ptrack_capability_policy::{confirm_approval, normalize};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::broker::{BrokerConfig, TOOL_HTTP_REQUEST, ToolCall};
use super::server::{
    BrokerClient, BrokerServer, BrokerServerConfig, SessionEnvironment, read_broker_descriptor,
    validate_session_environment,
};
use super::test_support::{TempDir, approved_http, refresh_approval, store_at};

pub(super) fn assert_cap_045_through_054_server_contract() {
    let environment = SessionEnvironment {
        token: "one-use-token".to_owned(),
        project: "/project".into(),
        generation: 7,
        profile: "agent-codex".to_owned(),
    };
    let variables = environment.variables();
    assert_eq!(variables.len(), 4);
    assert!(
        variables
            .iter()
            .any(|(name, _)| name == "PTRACK_CAPABILITY_TOKEN")
    );
    assert!(
        variables
            .iter()
            .any(|(name, _)| name == "PTRACK_CAPABILITY_PROFILE")
    );
}

#[tokio::test]
async fn loopback_request_gate_descriptor_and_client_fences_are_exact() {
    let temp = TempDir::new("broker-server");
    let home = temp.path().join("home");
    fs::create_dir(&home).unwrap();
    let (store, database, binding) = store_at(&temp);
    drop(store);
    let server = BrokerServer::start(BrokerServerConfig {
        global_home: home.clone(),
        broker: BrokerConfig {
            project_root: temp.path().to_path_buf(),
            database,
            binding,
            writer_version: "test".to_owned(),
            generation: 1,
        },
    })
    .unwrap();
    assert!(server.descriptor().url.starts_with("http://127.0.0.1:"));
    let descriptor_bytes = fs::read(server.descriptor_path()).unwrap();
    assert!(!String::from_utf8_lossy(&descriptor_bytes).contains("token"));
    assert_eq!(
        read_broker_descriptor(&home, temp.path()).unwrap(),
        *server.descriptor()
    );
    let token = server.broker().issue_session_token("agent-codex").unwrap();
    server.broker().bind_session(&token, "session").unwrap();
    super::http::ensure_tls_provider().unwrap();
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let unauthenticated = client
        .post(format!("{}/v1/tools/list", server.descriptor().url))
        .send()
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), 401);
    assert_eq!(
        unauthenticated.text().await.unwrap(),
        "capability session rejected\n"
    );
    let origin = client
        .post(format!("{}/v1/tools/list", server.descriptor().url))
        .bearer_auth(&token)
        .header("Origin", "http://hostile.invalid")
        .send()
        .await
        .unwrap();
    assert_eq!(origin.status(), 403);
    let valid = client
        .post(format!("{}/v1/tools/list", server.descriptor().url))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(valid.status(), 200);
    let value: Value = serde_json::from_slice(&valid.bytes().await.unwrap()).unwrap();
    assert_eq!(value["tools"].as_array().unwrap().len(), 3);
    assert_eq!(
        validate_session_environment(server.descriptor(), Some(temp.path()), Some("2"))
            .unwrap_err()
            .to_string(),
        "capability broker generation does not match the launched session"
    );
    server.shutdown().unwrap();
    assert!(!server.descriptor_path().exists());
}

#[tokio::test]
async fn active_handler_longer_than_idle_timeout_completes() {
    let target = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target_url = format!("http://{}/api", target.local_addr().unwrap());
    let target_task = tokio::spawn(async move {
        let (mut stream, _) = target.accept().await.unwrap();
        let mut request = Vec::new();
        let mut byte = [0_u8; 1];
        while !request.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).await.unwrap();
            request.push(byte[0]);
        }
        tokio::time::sleep(Duration::from_secs(16)).await;
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
            .await
            .unwrap();
    });

    let temp = TempDir::new("broker-active-over-idle");
    let home = temp.path().join("home");
    fs::create_dir(&home).unwrap();
    let (store, database, binding) = store_at(&temp);
    let mut capability = approved_http(&target_url);
    capability.limits.timeout_seconds = 30;
    capability = refresh_approval(capability);
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
    let server = BrokerServer::start(BrokerServerConfig {
        global_home: home,
        broker: BrokerConfig {
            project_root: temp.path().to_path_buf(),
            database,
            binding,
            writer_version: "test".to_owned(),
            generation: 1,
        },
    })
    .unwrap();
    let token = server.broker().issue_session_token("agent-codex").unwrap();
    server
        .broker()
        .bind_session(&token, "long-handler")
        .unwrap();
    let client = BrokerClient::new(server.descriptor().clone()).unwrap();
    let response = client
        .call(
            &token,
            &ToolCall {
                name: TOOL_HTTP_REQUEST.to_owned(),
                arguments: serde_json::json!({
                    "capability_id": capability.id,
                    "request": {"method": "GET", "url": target_url}
                }),
            },
        )
        .await
        .expect("active request must outlive the connection idle timeout");
    assert_eq!(response["status_code"], 200);
    assert_eq!(response["body"], "b2s=");
    target_task.await.unwrap();
    server.shutdown().unwrap();
}

#[test]
fn older_server_compare_remove_cannot_delete_replacement_descriptor() {
    let temp = TempDir::new("broker-compare-remove");
    let home = temp.path().join("home");
    fs::create_dir(&home).unwrap();
    let (store, database, binding) = store_at(&temp);
    drop(store);
    let server = BrokerServer::start(BrokerServerConfig {
        global_home: home.clone(),
        broker: BrokerConfig {
            project_root: temp.path().to_path_buf(),
            database,
            binding,
            writer_version: "test".to_owned(),
            generation: 1,
        },
    })
    .unwrap();
    let mut replacement = server.descriptor().clone();
    replacement.generation = 2;
    publish_runtime_json(
        &home,
        &server.descriptor().project_root,
        "capability-broker.json",
        &replacement,
    )
    .unwrap();
    assert_eq!(
        serde_json::from_slice::<super::BrokerDescriptor>(
            &fs::read(server.descriptor_path()).unwrap()
        )
        .unwrap(),
        replacement
    );
    server.shutdown().unwrap();
    assert_eq!(
        serde_json::from_slice::<super::BrokerDescriptor>(
            &fs::read(server.descriptor_path()).unwrap()
        )
        .unwrap(),
        replacement
    );
}
