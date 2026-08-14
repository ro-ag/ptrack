use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::{
    FlowLedger, MAX_CONTROL_FRAME_BYTES, OUTPUT_CHUNK_BYTES, OUTPUT_WINDOW_BYTES, ProtocolError,
    parse_ack_control, split_output,
};

#[test]
fn ack_control_accepts_only_strict_positive_integer_grammar() {
    assert_eq!(parse_ack_control(br#"{"type":"ack","bytes":1}"#), Ok(1));
    assert_eq!(
        parse_ack_control(br#" { "bytes" : 65536, "type" : "ack" } "#),
        Ok(65_536)
    );

    for payload in [
        br#"{"type":"ack","bytes":0}"#.as_slice(),
        br#"{"type":"ack","bytes":-1}"#,
        br#"{"type":"ack","bytes":+1}"#,
        br#"{"type":"ack","bytes":01}"#,
        br#"{"type":"ack","bytes":1.0}"#,
        br#"{"type":"ack","bytes":1e2}"#,
        br#"{"type":"ack","bytes":"1"}"#,
    ] {
        assert!(parse_ack_control(payload).is_err(), "accepted {payload:?}");
    }
}

#[test]
fn ack_control_rejects_malformed_duplicate_unknown_and_trailing_data() {
    for payload in [
        b"".as_slice(),
        b"[]",
        br#"{"type":"ack"}"#,
        br#"{"bytes":1}"#,
        br#"{"type":"detach","bytes":1}"#,
        br#"{"type":"ack","bytes":1,"extra":true}"#,
        br#"{"type":"ack","type":"ack","bytes":1}"#,
        br#"{"type":"ack","bytes":1,"bytes":1}"#,
        br#"{"type":"ack","bytes":1} null"#,
    ] {
        assert!(parse_ack_control(payload).is_err(), "accepted {payload:?}");
    }
    assert_eq!(
        parse_ack_control(&vec![b'x'; MAX_CONTROL_FRAME_BYTES + 1]),
        Err(ProtocolError::InvalidAckControlFrameSize)
    );
}

#[test]
fn split_output_never_emits_empty_or_oversized_chunks() {
    assert!(split_output(&[]).is_empty());
    let output = vec![42; OUTPUT_CHUNK_BYTES * 2 + 7];
    let chunks = split_output(&output);
    assert_eq!(
        chunks.iter().map(|chunk| chunk.len()).collect::<Vec<_>>(),
        [OUTPUT_CHUNK_BYTES, OUTPUT_CHUNK_BYTES, 7]
    );
    assert_eq!(chunks.concat(), output);
}

#[tokio::test]
async fn flow_ledger_keeps_one_maximum_chunk_in_flight_and_ack_resumes() {
    let ledger = Arc::new(FlowLedger::new(OUTPUT_WINDOW_BYTES));
    for _ in 0..(OUTPUT_WINDOW_BYTES / OUTPUT_CHUNK_BYTES - 1) {
        assert!(ledger.try_reserve_pending(OUTPUT_CHUNK_BYTES).await);
        ledger
            .commit(OUTPUT_CHUNK_BYTES, || async { Ok::<_, ()>(()) })
            .await
            .unwrap();
    }
    assert_eq!(
        ledger.unacknowledged().await,
        OUTPUT_WINDOW_BYTES - OUTPUT_CHUNK_BYTES
    );
    assert!(!ledger.try_reserve_pending(1).await);

    let cancellation = CancellationToken::new();
    let waiter = {
        let ledger = Arc::clone(&ledger);
        let cancellation = cancellation.clone();
        tokio::spawn(async move {
            ledger
                .reserve_pending(OUTPUT_CHUNK_BYTES, &cancellation)
                .await
        })
    };
    tokio::task::yield_now().await;
    ledger.acknowledge(OUTPUT_CHUNK_BYTES).await.unwrap();
    assert_eq!(waiter.await.unwrap(), Ok(()));
}

#[tokio::test]
async fn flow_ledger_rejects_ack_beyond_bytes_actually_sent() {
    let ledger = FlowLedger::new(OUTPUT_WINDOW_BYTES);
    assert!(ledger.try_reserve_pending(17).await);
    assert_eq!(
        ledger.acknowledge(1).await,
        Err(ProtocolError::AckExceedsBytesSent)
    );
    ledger
        .commit(17, || async { Ok::<_, ()>(()) })
        .await
        .unwrap();
    assert_eq!(
        ledger.acknowledge(18).await,
        Err(ProtocolError::AckExceedsBytesSent)
    );
    ledger.acknowledge(17).await.unwrap();
    assert_eq!(ledger.unacknowledged().await, 0);
}

#[tokio::test]
async fn concurrent_reservations_never_cross_the_bound() {
    let ledger = Arc::new(FlowLedger::new(OUTPUT_WINDOW_BYTES));
    let mut tasks = Vec::new();
    for _ in 0..64 {
        let ledger = Arc::clone(&ledger);
        tasks.push(tokio::spawn(async move {
            ledger.try_reserve_pending(OUTPUT_CHUNK_BYTES).await
        }));
    }
    let mut accepted = 0;
    for task in tasks {
        accepted += usize::from(task.await.unwrap());
    }
    assert_eq!(accepted, OUTPUT_WINDOW_BYTES / OUTPUT_CHUNK_BYTES - 1);
    assert_eq!(
        ledger.unacknowledged().await,
        OUTPUT_WINDOW_BYTES - OUTPUT_CHUNK_BYTES
    );
}
