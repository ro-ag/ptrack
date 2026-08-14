use std::fmt;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::{ToolCall, tool_definitions};
use tokio_util::sync::CancellationToken;

pub const MCP_PROTOCOL_VERSION: &str = "2025-11-25";
const MCP_PREVIOUS_PROTOCOL: &str = "2025-06-18";
const MAX_MCP_MESSAGE_BYTES: usize = 48 << 20;

const MCP_CANCEL_POLL: Duration = Duration::from_millis(20);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum McpServeOutcome {
    Complete,
    Cancelled,
}

/// Serves newline-delimited provider-compatible MCP until EOF or cancellation.
///
/// One process-wide reader slot bounds an input that cannot be interrupted by
/// portable synchronous I/O. Cancellation returns promptly even while the
/// owned reader remains blocked; that worker retains the sole slot until its
/// input closes, so repeated cancellation cannot accumulate reader threads.
///
/// # Errors
/// Returns only framing or output errors. Tool failures are encoded inside the
/// `tools/call` result as required by MCP.
pub fn serve_mcp<E: fmt::Display>(
    input: Box<dyn Read + Send>,
    output: &mut dyn Write,
    cancellation: &CancellationToken,
    mut call: impl FnMut(&CancellationToken, ToolCall) -> Result<Value, E>,
) -> Result<McpServeOutcome, McpError> {
    let Some(reader_slot) = acquire_reader_slot(cancellation) else {
        return Ok(McpServeOutcome::Cancelled);
    };
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("ptrack-capability-mcp-reader".to_owned())
        .spawn(move || read_frames(input, &sender, reader_slot))
        .map_err(|_| McpError("MCP reader is unavailable".to_owned()))?;
    let mut initialized = false;
    loop {
        let Some(line) = next_frame(&receiver, cancellation)? else {
            return Ok(if cancellation.is_cancelled() {
                McpServeOutcome::Cancelled
            } else {
                McpServeOutcome::Complete
            });
        };
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let request = serde_json::from_slice::<McpRequest>(&line);
        let response = match request {
            Ok(request) if request.jsonrpc == "2.0" && !request.method.is_empty() => {
                handle_request(request, &mut initialized, cancellation, &mut call)
            }
            _ => Some(McpResponse::error(None, -32700, "parse error")),
        };
        if cancellation.is_cancelled() {
            return Ok(McpServeOutcome::Cancelled);
        }
        if let Some(response) = response {
            serde_json::to_writer(&mut *output, &response)
                .map_err(|_| McpError("write MCP response".to_owned()))?;
            output
                .write_all(b"\n")
                .map_err(|_| McpError("write MCP response".to_owned()))?;
            output
                .flush()
                .map_err(|_| McpError("write MCP response".to_owned()))?;
        }
    }
}

#[derive(Deserialize)]
struct McpRequest {
    jsonrpc: String,
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Option<Value>,
}

#[derive(Serialize)]
struct McpResponse {
    jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<McpResponseError>,
}

#[derive(Serialize)]
struct McpResponseError {
    code: i32,
    message: &'static str,
}

impl McpResponse {
    fn result(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    fn error(id: Option<Value>, code: i32, message: &'static str) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(McpResponseError { code, message }),
        }
    }
}

fn handle_request<E: fmt::Display>(
    request: McpRequest,
    initialized: &mut bool,
    cancellation: &CancellationToken,
    call: &mut impl FnMut(&CancellationToken, ToolCall) -> Result<Value, E>,
) -> Option<McpResponse> {
    let notification = request.id.is_none();
    let id = request.id;
    match request.method.as_str() {
        "initialize" => handle_initialize(id, notification, request.params, initialized),
        "notifications/initialized" => None,
        "ping" => {
            if notification {
                None
            } else {
                Some(McpResponse::result(id, json!({})))
            }
        }
        "tools/list" => {
            if notification {
                return None;
            }
            if !*initialized {
                return Some(McpResponse::error(id, -32002, "server is not initialized"));
            }
            Some(McpResponse::result(
                id,
                json!({"tools": tool_definitions()}),
            ))
        }
        "tools/call" => handle_tool_call(
            id,
            notification,
            request.params,
            *initialized,
            cancellation,
            call,
        ),
        _ if notification => None,
        _ => Some(McpResponse::error(id, -32601, "method not found")),
    }
}

fn handle_initialize(
    id: Option<Value>,
    notification: bool,
    params: Option<Value>,
    initialized: &mut bool,
) -> Option<McpResponse> {
    if notification {
        return None;
    }
    let Some(params) = params else {
        return Some(invalid_initialize(id));
    };
    let Ok(params) = serde_json::from_value::<InitializeParams>(params) else {
        return Some(invalid_initialize(id));
    };
    let protocol = if params.protocol_version == MCP_PREVIOUS_PROTOCOL {
        MCP_PREVIOUS_PROTOCOL
    } else {
        MCP_PROTOCOL_VERSION
    };
    *initialized = true;
    Some(McpResponse::result(
        id,
        json!({
            "protocolVersion": protocol,
            "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": {"name": "p-track-capabilities", "version": "1"}
        }),
    ))
}

fn invalid_initialize(id: Option<Value>) -> McpResponse {
    McpResponse::error(id, -32602, "invalid initialize parameters")
}

fn handle_tool_call<E: fmt::Display>(
    id: Option<Value>,
    notification: bool,
    params: Option<Value>,
    initialized: bool,
    cancellation: &CancellationToken,
    call: &mut impl FnMut(&CancellationToken, ToolCall) -> Result<Value, E>,
) -> Option<McpResponse> {
    if notification {
        return None;
    }
    if !initialized {
        return Some(McpResponse::error(id, -32002, "server is not initialized"));
    }
    let params = params
        .and_then(|params| serde_json::from_value::<ToolParams>(params).ok())
        .filter(|params| {
            tool_definitions()
                .iter()
                .any(|tool| tool.name == params.name)
        });
    let Some(params) = params else {
        return Some(McpResponse::error(
            id,
            -32602,
            "unknown tool or invalid parameters",
        ));
    };
    let result = call(
        cancellation,
        ToolCall {
            name: params.name,
            arguments: Value::Object(params.arguments),
        },
    )
    .map_or_else(
        |error| {
            json!({
                "content": [{"type": "text", "text": error.to_string()}],
                "isError": true
            })
        },
        successful_tool_result,
    );
    Some(McpResponse::result(id, result))
}

fn successful_tool_result(value: Value) -> Value {
    let text = serde_json::to_string(&value).unwrap_or_else(|_| "null".to_owned());
    let mut result = json!({
        "content": [{"type": "text", "text": text}],
        "isError": false
    });
    if value.is_object()
        && let Some(object) = result.as_object_mut()
    {
        object.insert("structuredContent".to_owned(), value);
    }
    result
}

#[derive(Deserialize)]
struct InitializeParams {
    #[serde(rename = "protocolVersion")]
    protocol_version: String,
}

#[derive(Deserialize)]
struct ToolParams {
    name: String,
    arguments: Map<String, Value>,
}

fn read_limited_line(input: &mut impl BufRead) -> Result<Option<Vec<u8>>, McpError> {
    let mut line = Vec::with_capacity(64 << 10);
    loop {
        let available = input
            .fill_buf()
            .map_err(|_| McpError("read MCP request".to_owned()))?;
        if available.is_empty() {
            return if line.is_empty() {
                Ok(None)
            } else {
                Ok(Some(line))
            };
        }
        let take = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |index| index + 1);
        if line.len().saturating_add(take) > MAX_MCP_MESSAGE_BYTES {
            return Err(McpError("MCP request exceeds its byte limit".to_owned()));
        }
        line.extend_from_slice(&available[..take]);
        input.consume(take);
        if line.last() == Some(&b'\n') {
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            return Ok(Some(line));
        }
    }
}

enum ReaderFrame {
    Line(Vec<u8>),
    Complete,
    Error(McpError),
}

fn read_frames(
    input: Box<dyn Read + Send>,
    sender: &SyncSender<ReaderFrame>,
    _reader_slot: ReaderSlot,
) {
    let mut input = BufReader::with_capacity(64 << 10, input);
    loop {
        let frame = match read_limited_line(&mut input) {
            Ok(Some(line)) => ReaderFrame::Line(line),
            Ok(None) => ReaderFrame::Complete,
            Err(error) => ReaderFrame::Error(error),
        };
        let finished = !matches!(frame, ReaderFrame::Line(_));
        if sender.send(frame).is_err() || finished {
            return;
        }
    }
}

fn next_frame(
    receiver: &Receiver<ReaderFrame>,
    cancellation: &CancellationToken,
) -> Result<Option<Vec<u8>>, McpError> {
    loop {
        if cancellation.is_cancelled() {
            return Ok(None);
        }
        match receiver.recv_timeout(MCP_CANCEL_POLL) {
            Ok(ReaderFrame::Line(line)) => {
                if cancellation.is_cancelled() {
                    return Ok(None);
                }
                return Ok(Some(line));
            }
            Ok(ReaderFrame::Complete) => return Ok(None),
            Ok(ReaderFrame::Error(error)) => return Err(error),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                return Err(McpError("read MCP request".to_owned()));
            }
        }
    }
}

fn reader_slot() -> &'static (Mutex<bool>, Condvar) {
    static SLOT: OnceLock<(Mutex<bool>, Condvar)> = OnceLock::new();
    SLOT.get_or_init(|| (Mutex::new(false), Condvar::new()))
}

fn acquire_reader_slot(cancellation: &CancellationToken) -> Option<ReaderSlot> {
    let (occupied, available) = reader_slot();
    let mut occupied = occupied
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    while *occupied {
        if cancellation.is_cancelled() {
            return None;
        }
        occupied = available
            .wait_timeout(occupied, MCP_CANCEL_POLL)
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .0;
    }
    if cancellation.is_cancelled() {
        return None;
    }
    *occupied = true;
    Some(ReaderSlot)
}

struct ReaderSlot;

impl Drop for ReaderSlot {
    fn drop(&mut self) {
        let (occupied, available) = reader_slot();
        *occupied
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = false;
        available.notify_one();
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct McpError(String);

impl fmt::Display for McpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for McpError {}
