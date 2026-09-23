use std::io::Write as _;
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{Value, json};

use crate::mcp_transport::{
    MCP_PROTOCOL_VERSION, McpCancellation, McpOutcome, ToolDefinition, serve_mcp_with_tools,
};

fn tools() -> Vec<ToolDefinition> {
    vec![ToolDefinition {
        name: "get_context".to_owned(),
        title: "Get context".to_owned(),
        description: "Read context".to_owned(),
        input_schema: json!({"type":"object","additionalProperties":false}),
        annotations: json!({"readOnlyHint":true}),
    }]
}

fn rows(output: Vec<u8>) -> Vec<Value> {
    String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn generic_mcp_surface_uses_supplied_identity_definitions_and_handler() {
    let input = br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25"}}
{"jsonrpc":"2.0","id":2,"method":"tools/list"}
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_context","arguments":{}}}
"#
    .to_vec();
    let mut output = Vec::new();
    let outcome = serve_mcp_with_tools(
        Box::new(std::io::Cursor::new(input)),
        &mut output,
        &McpCancellation::new(),
        "p-track-project",
        "1",
        &tools(),
        |_, call| Ok::<_, &'static str>(json!({"called": call.name})),
    )
    .unwrap();
    assert_eq!(outcome, McpOutcome::Complete);
    let rows = rows(output);
    assert_eq!(rows[0]["result"]["protocolVersion"], MCP_PROTOCOL_VERSION);
    assert_eq!(rows[0]["result"]["serverInfo"]["name"], "p-track-project");
    assert_eq!(rows[0]["result"]["serverInfo"]["version"], "1");
    assert_eq!(rows[1]["result"]["tools"].as_array().unwrap().len(), 1);
    assert_eq!(rows[1]["result"]["tools"][0]["name"], "get_context");
    assert_eq!(
        rows[2]["result"]["structuredContent"]["called"],
        "get_context"
    );
    assert_eq!(rows[2]["result"]["isError"], false);
}

#[test]
fn mcp_negotiates_the_previous_protocol_and_answers_ping() {
    let input = br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}
{"jsonrpc":"2.0","id":2,"method":"ping"}
{"jsonrpc":"2.0","method":"ping"}
{"jsonrpc":"2.0","id":3,"method":"initialize","params":{"protocolVersion":"1999-01-01"}}
"#
    .to_vec();
    let mut output = Vec::new();
    serve_mcp_with_tools(
        Box::new(std::io::Cursor::new(input)),
        &mut output,
        &McpCancellation::new(),
        "p-track-project",
        "1",
        &tools(),
        |_, _| Ok::<_, &'static str>(json!({})),
    )
    .unwrap();
    let rows = rows(output);
    assert_eq!(rows.len(), 3, "a ping notification gets no response");
    assert_eq!(rows[0]["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(rows[1]["result"], json!({}));
    assert_eq!(rows[2]["result"]["protocolVersion"], MCP_PROTOCOL_VERSION);
}

#[test]
fn mcp_parse_preinit_notifications_unknown_and_tool_errors_are_exact() {
    let input = br#"bad
{"jsonrpc":"2.0","id":1,"method":"tools/list"}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"2025-11-25"}}
{"jsonrpc":"2.0","id":3,"method":"unknown"}
{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"get_context","arguments":{}}}
{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"not_a_tool","arguments":{}}}
{"jsonrpc":"2.0","id":6,"method":"initialize"}
"#
    .to_vec();
    let mut output = Vec::new();
    serve_mcp_with_tools(
        Box::new(std::io::Cursor::new(input)),
        &mut output,
        &McpCancellation::new(),
        "p-track-project",
        "1",
        &tools(),
        |_, _| Err::<Value, _>("denied safely"),
    )
    .unwrap();
    let rows = rows(output);
    assert_eq!(rows.len(), 7);
    assert_eq!(rows[0]["error"]["code"], -32700);
    assert_eq!(rows[1]["error"]["code"], -32002);
    assert_eq!(rows[3]["error"]["code"], -32601);
    assert_eq!(rows[4]["result"]["isError"], true);
    assert_eq!(rows[4]["result"]["content"][0]["text"], "denied safely");
    assert_eq!(rows[5]["error"]["code"], -32602);
    assert_eq!(rows[6]["error"]["code"], -32602);
}

#[test]
fn mcp_cancellation_returns_while_owned_input_remains_open() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut peer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (input, _) = listener.accept().unwrap();
    let cancellation = McpCancellation::new();
    let worker_cancellation = cancellation.clone();
    let (result_tx, result_rx) = mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        let mut output = Vec::new();
        let outcome = serve_mcp_with_tools(
            Box::new(input),
            &mut output,
            &worker_cancellation,
            "p-track-project",
            "1",
            &tools(),
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
    assert_eq!(outcome.unwrap(), McpOutcome::Cancelled);
    assert!(output.is_empty());

    drop(peer);
    worker.join().unwrap();
}
