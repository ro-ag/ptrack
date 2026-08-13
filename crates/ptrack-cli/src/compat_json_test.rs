use serde::Serialize;

use crate::compat_json::timestamp;
use crate::compat_json::{DigestJson, raw_or_null};

#[derive(Serialize)]
struct Escaped<'a> {
    value: &'a str,
}

#[test]
fn json_matches_go_html_and_separator_escaping() {
    let mut output = Vec::new();
    crate::output::json(
        &mut output,
        &Escaped {
            value: "<tag>&\u{2028}\u{2029}",
        },
    )
    .expect("encode");
    assert_eq!(
        String::from_utf8(output).expect("utf8"),
        "{\n  \"value\": \"\\u003ctag\\u003e\\u0026\\u2028\\u2029\"\n}\n"
    );
}

#[test]
fn timestamp_uses_go_rfc3339_nano_shape() {
    assert_eq!(
        timestamp(ptrack_core::Timestamp::Zero),
        "0001-01-01T00:00:00Z"
    );
    assert_eq!(
        timestamp(ptrack_core::Timestamp::Fixed {
            seconds: 1_786_547_696,
            nanoseconds: 120_000_000,
            offset_seconds: -7 * 3_600,
        }),
        "2026-08-12T08:14:56.12-07:00"
    );
}

#[test]
fn empty_go_nil_slices_encode_as_null_while_derived_rows_can_remain_arrays() {
    let snapshot = ptrack_core::ProjectSnapshot::new(
        ptrack_core::Meta {
            goal: String::new(),
            summary: String::new(),
            active_plan: 0,
            created_at: ptrack_core::Timestamp::Zero,
            updated_at: ptrack_core::Timestamp::Zero,
            format_version: 5,
            last_write_version: String::new(),
        },
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    let digest = ptrack_core::context(&snapshot);
    let encoded = serde_json::to_string(&DigestJson::from(&digest)).expect("digest json");
    assert!(encoded.contains("\"blocked\":null"));
    assert!(encoded.contains("\"open_issues\":null"));
    assert!(encoded.contains("\"recent_notes\":null"));
    assert_eq!(
        serde_json::to_string(&raw_or_null::<u8>(Vec::new())).expect("raw list"),
        "null"
    );
    assert_eq!(
        serde_json::to_string(&Vec::<u8>::new()).expect("derived list"),
        "[]"
    );
}
