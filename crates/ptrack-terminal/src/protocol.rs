//! Terminal WebSocket framing and acknowledgement flow control.

use std::future::Future;

use serde::Deserialize;
use serde_json::Value;
use tokio::sync::{Mutex, Notify};
use tokio_util::sync::CancellationToken;

/// Largest terminal output frame emitted by the stream server.
pub const OUTPUT_CHUNK_BYTES: usize = 64 * 1024;
/// Maximum amount of terminal output reserved but not acknowledged.
pub const OUTPUT_WINDOW_BYTES: usize = 512 * 1024;
/// Largest accepted text control frame.
pub const MAX_CONTROL_FRAME_BYTES: usize = 1024;
/// Largest accepted terminal input frame.
pub const MAX_INPUT_FRAME_BYTES: usize = 64 * 1024;

/// Errors in the terminal wire protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    InvalidAckControlFrameSize,
    AckControlFrameMustBeObject,
    InvalidAckControlField,
    InvalidAckControlFieldName,
    DuplicateAckControlType,
    AckControlTypeMustBeString,
    DuplicateAckControlByteCount,
    InvalidAckControlByteCount,
    AckByteCountMustBePositiveInteger,
    AckByteCountOutOfRange,
    UnknownAckControlField(String),
    InvalidAckControlObject,
    InvalidAckControlFrame,
    TrailingAckControlData,
    AckByteCountMustBePositive,
    AckExceedsBytesSent,
    InvalidReservation,
    Cancelled,
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidAckControlFrameSize => {
                formatter.write_str("invalid ACK control frame size")
            }
            Self::AckControlFrameMustBeObject => {
                formatter.write_str("ACK control frame must be an object")
            }
            Self::InvalidAckControlField => formatter.write_str("invalid ACK control field"),
            Self::InvalidAckControlFieldName => {
                formatter.write_str("invalid ACK control field name")
            }
            Self::DuplicateAckControlType => formatter.write_str("duplicate ACK control type"),
            Self::AckControlTypeMustBeString => {
                formatter.write_str("ACK control type must be a string")
            }
            Self::DuplicateAckControlByteCount => {
                formatter.write_str("duplicate ACK control byte count")
            }
            Self::InvalidAckControlByteCount => {
                formatter.write_str("invalid ACK control byte count")
            }
            Self::AckByteCountMustBePositiveInteger => {
                formatter.write_str("ACK byte count must be a positive integer")
            }
            Self::AckByteCountOutOfRange => formatter.write_str("ACK byte count is out of range"),
            Self::UnknownAckControlField(field) => {
                write!(formatter, "unknown ACK control field {field:?}")
            }
            Self::InvalidAckControlObject => formatter.write_str("invalid ACK control object"),
            Self::InvalidAckControlFrame => formatter.write_str("invalid ACK control frame"),
            Self::TrailingAckControlData => formatter.write_str("trailing ACK control data"),
            Self::AckByteCountMustBePositive => {
                formatter.write_str("ACK byte count must be positive")
            }
            Self::AckExceedsBytesSent => formatter.write_str("ACK exceeds bytes sent"),
            Self::InvalidReservation => formatter.write_str("invalid terminal output reservation"),
            Self::Cancelled => formatter.write_str("terminal stream cancelled"),
        }
    }
}

impl std::error::Error for ProtocolError {}

/// Parse the deliberately tiny and strict ACK grammar.
///
/// # Errors
///
/// Returns a protocol error unless the payload is exactly one strict ACK
/// object with a positive base-10 integer byte count.
pub fn parse_ack_control(payload: &[u8]) -> Result<usize, ProtocolError> {
    if payload.is_empty() || payload.len() > MAX_CONTROL_FRAME_BYTES {
        return Err(ProtocolError::InvalidAckControlFrameSize);
    }

    let mut deserializer = serde_json::Deserializer::from_slice(payload);
    let value = Value::deserialize(&mut deserializer)
        .map_err(|_| ProtocolError::AckControlFrameMustBeObject)?;
    deserializer
        .end()
        .map_err(|_| ProtocolError::TrailingAckControlData)?;

    let object = value
        .as_object()
        .ok_or(ProtocolError::AckControlFrameMustBeObject)?;
    // `serde_json::Map` cannot retain duplicate names. Scan the original object so
    // duplicate authority-bearing fields are rejected rather than last-one-wins.
    reject_duplicate_ack_fields(payload)?;

    for key in object.keys() {
        if key != "type" && key != "bytes" {
            return Err(ProtocolError::UnknownAckControlField(key.clone()));
        }
    }
    let control_type = object
        .get("type")
        .ok_or(ProtocolError::InvalidAckControlFrame)?
        .as_str()
        .ok_or(ProtocolError::AckControlTypeMustBeString)?;
    let byte_value = object
        .get("bytes")
        .ok_or(ProtocolError::InvalidAckControlFrame)?;
    if control_type != "ack" {
        return Err(ProtocolError::InvalidAckControlFrame);
    }

    // Parsing the raw token is intentional: serde_json's numeric representation
    // would otherwise accept exponent and fractional spellings of integral values.
    let raw_bytes =
        raw_object_value(payload, "bytes").ok_or(ProtocolError::InvalidAckControlByteCount)?;
    if !is_strict_positive_decimal(raw_bytes) {
        return Err(ProtocolError::AckByteCountMustBePositiveInteger);
    }
    if !byte_value.is_number() {
        return Err(ProtocolError::AckByteCountMustBePositiveInteger);
    }
    raw_bytes
        .parse::<usize>()
        .map_err(|_| ProtocolError::AckByteCountOutOfRange)
}

fn reject_duplicate_ack_fields(payload: &[u8]) -> Result<(), ProtocolError> {
    let mut type_count = 0;
    let mut bytes_count = 0;
    let mut stream = serde_json::Deserializer::from_slice(payload).into_iter::<Value>();
    // The strict duplicate scan below is lexical and string-aware. The stream call
    // first guarantees that escapes and nesting are valid JSON.
    stream
        .next()
        .ok_or(ProtocolError::InvalidAckControlObject)?
        .map_err(|_| ProtocolError::InvalidAckControlObject)?;
    let text = std::str::from_utf8(payload).map_err(|_| ProtocolError::InvalidAckControlObject)?;
    for key in top_level_object_keys(text)? {
        match key.as_str() {
            "type" => type_count += 1,
            "bytes" => bytes_count += 1,
            _ => {}
        }
    }
    if type_count > 1 {
        return Err(ProtocolError::DuplicateAckControlType);
    }
    if bytes_count > 1 {
        return Err(ProtocolError::DuplicateAckControlByteCount);
    }
    Ok(())
}

fn top_level_object_keys(text: &str) -> Result<Vec<String>, ProtocolError> {
    let bytes = text.as_bytes();
    let mut keys = Vec::new();
    let mut index = skip_space(bytes, 0);
    if bytes.get(index) != Some(&b'{') {
        return Err(ProtocolError::AckControlFrameMustBeObject);
    }
    index += 1;
    loop {
        index = skip_space(bytes, index);
        if bytes.get(index) == Some(&b'}') {
            return Ok(keys);
        }
        let (key, next) = parse_json_string(bytes, index)?;
        keys.push(key);
        index = skip_space(bytes, next);
        if bytes.get(index) != Some(&b':') {
            return Err(ProtocolError::InvalidAckControlField);
        }
        index = skip_json_value(bytes, skip_space(bytes, index + 1));
        index = skip_space(bytes, index);
        match bytes.get(index) {
            Some(b',') => index += 1,
            Some(b'}') => return Ok(keys),
            _ => return Err(ProtocolError::InvalidAckControlObject),
        }
    }
}

fn raw_object_value<'a>(payload: &'a [u8], wanted: &str) -> Option<&'a str> {
    let text = std::str::from_utf8(payload).ok()?;
    let bytes = text.as_bytes();
    let object_start = skip_space(bytes, 0);
    if bytes.get(object_start) != Some(&b'{') {
        return None;
    }
    let mut index = skip_space(bytes, object_start + 1);
    while bytes.get(index) != Some(&b'}') {
        let (key, next) = parse_json_string(bytes, index).ok()?;
        index = skip_space(bytes, next);
        if bytes.get(index) != Some(&b':') {
            return None;
        }
        let start = skip_space(bytes, index + 1);
        let end = skip_json_value(bytes, start);
        if key == wanted {
            return text.get(start..end);
        }
        index = skip_space(bytes, end);
        match bytes.get(index) {
            Some(b',') => index = skip_space(bytes, index + 1),
            Some(b'}') => break,
            _ => return None,
        }
    }
    None
}

fn skip_space(bytes: &[u8], mut index: usize) -> usize {
    while matches!(bytes.get(index), Some(b' ' | b'\n' | b'\r' | b'\t')) {
        index += 1;
    }
    index
}

fn parse_json_string(bytes: &[u8], start: usize) -> Result<(String, usize), ProtocolError> {
    if bytes.get(start) != Some(&b'"') {
        return Err(ProtocolError::InvalidAckControlFieldName);
    }
    let mut index = start + 1;
    while let Some(byte) = bytes.get(index) {
        match byte {
            b'"' => {
                let encoded = std::str::from_utf8(&bytes[start..=index])
                    .map_err(|_| ProtocolError::InvalidAckControlFieldName)?;
                let decoded = serde_json::from_str(encoded)
                    .map_err(|_| ProtocolError::InvalidAckControlFieldName)?;
                return Ok((decoded, index + 1));
            }
            b'\\' => index += 2,
            _ => index += 1,
        }
    }
    Err(ProtocolError::InvalidAckControlFieldName)
}

fn skip_json_value(bytes: &[u8], start: usize) -> usize {
    let mut index = start;
    let mut depth = 0_usize;
    let mut in_string = false;
    let mut escaped = false;
    while let Some(&byte) = bytes.get(index) {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            index += 1;
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' | b'[' => depth += 1,
            b'}' | b']' if depth > 0 => depth -= 1,
            b',' | b'}' if depth == 0 => return index,
            _ => {}
        }
        index += 1;
    }
    index
}

fn is_strict_positive_decimal(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && matches!(bytes[0], b'1'..=b'9')
        && bytes[1..].iter().all(u8::is_ascii_digit)
}

#[derive(Debug, Default)]
struct LedgerState {
    reserved: usize,
    sent: usize,
}

/// Tracks output reserved, sent, and acknowledged for one stream connection.
#[derive(Debug)]
pub struct FlowLedger {
    window: usize,
    state: Mutex<LedgerState>,
    changed: Notify,
}

impl FlowLedger {
    #[must_use]
    pub fn new(window: usize) -> Self {
        Self {
            window,
            state: Mutex::new(LedgerState::default()),
            changed: Notify::new(),
        }
    }

    pub async fn try_reserve_pending(&self, byte_count: usize) -> bool {
        if byte_count == 0 || byte_count > OUTPUT_CHUNK_BYTES {
            return false;
        }
        let Some(limit) = self.window.checked_sub(OUTPUT_CHUNK_BYTES) else {
            return false;
        };
        let mut state = self.state.lock().await;
        if state
            .reserved
            .checked_add(byte_count)
            .is_none_or(|sum| sum > limit)
        {
            return false;
        }
        state.reserved += byte_count;
        true
    }

    /// Wait until the requested output capacity can be reserved.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid reservation or when cancelled.
    pub async fn reserve_pending(
        &self,
        byte_count: usize,
        cancellation: &CancellationToken,
    ) -> Result<(), ProtocolError> {
        if byte_count == 0 || byte_count > OUTPUT_CHUNK_BYTES {
            return Err(ProtocolError::InvalidReservation);
        }
        loop {
            let notified = self.changed.notified();
            if self.try_reserve_pending(byte_count).await {
                return Ok(());
            }
            tokio::select! {
                () = notified => {}
                () = cancellation.cancelled() => return Err(ProtocolError::Cancelled),
            }
        }
    }

    /// Keep the ledger locked through the actual send so an ACK cannot race
    /// ahead of the bytes becoming observable on the wire.
    ///
    /// # Errors
    ///
    /// Returns the error produced by `send` without marking bytes as sent.
    pub async fn commit<F, Fut, E>(&self, byte_count: usize, send: F) -> Result<(), E>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<(), E>>,
    {
        let mut state = self.state.lock().await;
        send().await?;
        state.sent += byte_count;
        Ok(())
    }

    /// Release an exact number of bytes that were already sent.
    ///
    /// # Errors
    ///
    /// Returns an error for zero or for an ACK beyond bytes actually sent.
    pub async fn acknowledge(&self, byte_count: usize) -> Result<(), ProtocolError> {
        if byte_count == 0 {
            return Err(ProtocolError::AckByteCountMustBePositive);
        }
        let mut state = self.state.lock().await;
        if byte_count > state.sent {
            return Err(ProtocolError::AckExceedsBytesSent);
        }
        state.reserved -= byte_count;
        state.sent -= byte_count;
        drop(state);
        // There is one output writer per stream. `notify_one` also retains a
        // permit if the writer has not polled its wait future yet.
        self.changed.notify_one();
        Ok(())
    }

    pub async fn release(&self, byte_count: usize) {
        if byte_count == 0 {
            return;
        }
        let mut state = self.state.lock().await;
        state.reserved = state.reserved.saturating_sub(byte_count);
        drop(state);
        self.changed.notify_one();
    }

    #[must_use]
    pub async fn unacknowledged(&self) -> usize {
        self.state.lock().await.reserved
    }
}

/// Split output into non-empty frames no larger than 64 KiB.
#[must_use]
pub fn split_output(output: &[u8]) -> Vec<&[u8]> {
    output.chunks(OUTPUT_CHUNK_BYTES).collect()
}
