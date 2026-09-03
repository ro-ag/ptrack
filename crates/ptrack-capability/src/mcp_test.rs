use std::io::Write as _;
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::mcp::{McpServeOutcome, serve_mcp, serve_mcp_with_tools};
use crate::ToolDefinition;

pub(super) fn assert_cap_085_through_087_mcp_contract() {
    let input = br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}
{"jsonrpc":"2.0","id":2,"method":"tools/list"}
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"ptrack_http_request","arguments":{}}}
"#
    .to_vec();
    let mut output = Vec::new();
    assert_eq!(
        serve_mcp(
            Box::new(std::io::Cursor::new(input)),
            &mut output,
            &CancellationToken::new(),
            |_, _| { Ok::<_, &'static str>(json!({"ok": true})) },
        )
        .unwrap(),
        McpServeOutcome::Complete
    );
    let rows: Vec<Value> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows[0]["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(rows[1]["result"]["tools"].as_array().unwrap().len(), 3);
    assert_eq!(rows[2]["result"]["structuredContent"]["ok"], true);
    assert_eq!(rows[2]["result"]["isError"], false);
}

#[test]
fn generic_mcp_surface_uses_supplied_identity_definitions_and_handler() {
    let tools = [ToolDefinition {
        name: "get_context".to_owned(),
        title: "Get context".to_owned(),
        description: "Read context".to_owned(),
        input_schema: json!({"type":"object","additionalProperties":false}),
        annotations: json!({"readOnlyHint":true}),
    }];
    let input = br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25"}}
{"jsonrpc":"2.0","id":2,"method":"tools/list"}
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_context","arguments":{}}}
"#
    .to_vec();
    let mut output = Vec::new();
    serve_mcp_with_tools(
        Box::new(std::io::Cursor::new(input)),
        &mut output,
        &CancellationToken::new(),
        "p-track-project",
        "1",
        &tools,
        |_, call| Ok::<_, &'static str>(json!({"called": call.name})),
    )
    .unwrap();
    let rows: Vec<Value> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows[0]["result"]["serverInfo"]["name"], "p-track-project");
    assert_eq!(rows[1]["result"]["tools"].as_array().unwrap().len(), 1);
    assert_eq!(rows[1]["result"]["tools"][0]["name"], "get_context");
    assert_eq!(
        rows[2]["result"]["structuredContent"]["called"],
        "get_context"
    );
}

#[test]
fn mcp_parse_preinit_notifications_unknown_and_tool_errors_are_exact() {
    let input = br#"bad
{"jsonrpc":"2.0","id":1,"method":"tools/list"}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"2025-11-25"}}
{"jsonrpc":"2.0","id":3,"method":"unknown"}
{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"ptrack_git","arguments":{}}}
"#
    .to_vec();
    let mut output = Vec::new();
    serve_mcp(
        Box::new(std::io::Cursor::new(input)),
        &mut output,
        &CancellationToken::new(),
        |_, _| Err::<Value, _>("denied safely"),
    )
    .unwrap();
    let rows: Vec<Value> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows.len(), 5);
    assert_eq!(rows[0]["error"]["code"], -32700);
    assert_eq!(rows[1]["error"]["code"], -32002);
    assert_eq!(rows[3]["error"]["code"], -32601);
    assert_eq!(rows[4]["result"]["isError"], true);
    assert_eq!(rows[4]["result"]["content"][0]["text"], "denied safely");
}

#[test]
fn mcp_cancellation_returns_while_owned_input_remains_open() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut peer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (input, _) = listener.accept().unwrap();
    let cancellation = CancellationToken::new();
    let worker_cancellation = cancellation.clone();
    let (result_tx, result_rx) = mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        let mut output = Vec::new();
        let outcome = serve_mcp(
            Box::new(input),
            &mut output,
            &worker_cancellation,
            |_, _| Ok::<_, &'static str>(json!({})),
        );
        result_tx.send((outcome, output)).unwrap();
    });

    peer.write_all(b" ").unwrap();
    std::thread::sleep(Duration::from_millis(50));
    cancellation.cancel();
    let (outcome, output) = result_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("MCP cancellation stayed blocked on open input");
    assert_eq!(outcome.unwrap(), McpServeOutcome::Cancelled);
    assert!(output.is_empty());

    drop(peer);
    worker.join().unwrap();
}
