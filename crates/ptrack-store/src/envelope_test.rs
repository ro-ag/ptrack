use super::{EnvelopeError, RECORD_ENVELOPE_VERSION, RecordEnvelope};

#[test]
fn envelope_v1_has_a_stable_big_endian_layout() {
    let encoded = RecordEnvelope::new(0x1234, 0x0102_0304, [0xde, 0xad]).encode();

    assert_eq!(
        encoded,
        [
            b'P', b'T', b'R', b'K', // magic
            0x00, 0x01, // envelope version
            0x12, 0x34, // codec
            0x01, 0x02, 0x03, 0x04, // payload schema
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, // payload length
            0xde, 0xad,
        ]
    );
}

#[test]
fn arbitrary_payload_and_unknown_codec_round_trip_exactly() {
    let original = RecordEnvelope::new(u16::MAX, 42, [0x00, 0xff, 0x80, b'P', b'T', b'R', b'K']);

    let decoded = RecordEnvelope::decode(&original.encode()).unwrap();

    assert_eq!(decoded, original);
    assert_eq!(decoded.codec(), u16::MAX);
    assert_eq!(decoded.payload_schema(), 42);
    assert_eq!(
        decoded.payload(),
        [0x00, 0xff, 0x80, b'P', b'T', b'R', b'K']
    );
    assert_eq!(
        decoded.into_payload(),
        [0x00, 0xff, 0x80, b'P', b'T', b'R', b'K']
    );
}

#[test]
fn empty_payload_round_trips() {
    let original = RecordEnvelope::new(0, 0, Vec::new());

    assert_eq!(
        RecordEnvelope::decode(&original.encode()).unwrap(),
        original
    );
}

#[test]
fn truncated_header_is_rejected() {
    let error = RecordEnvelope::decode(b"PTRK\0\x01").unwrap_err();

    assert_eq!(
        error,
        EnvelopeError::HeaderTooShort {
            actual: 6,
            minimum: 20,
        }
    );
}

#[test]
fn invalid_magic_is_rejected() {
    let mut encoded = RecordEnvelope::new(1, 1, []).encode();
    encoded[0..4].copy_from_slice(b"NOPE");

    assert_eq!(
        RecordEnvelope::decode(&encoded).unwrap_err(),
        EnvelopeError::InvalidMagic { actual: *b"NOPE" }
    );
}

#[test]
fn unsupported_envelope_version_is_rejected() {
    let mut encoded = RecordEnvelope::new(1, 1, []).encode();
    encoded[4..6].copy_from_slice(&(RECORD_ENVELOPE_VERSION + 1).to_be_bytes());

    assert_eq!(
        RecordEnvelope::decode(&encoded).unwrap_err(),
        EnvelopeError::UnsupportedEnvelopeVersion {
            actual: RECORD_ENVELOPE_VERSION + 1,
            supported: RECORD_ENVELOPE_VERSION,
        }
    );
}

#[test]
fn truncated_payload_is_rejected() {
    let mut encoded = RecordEnvelope::new(1, 1, [0xaa, 0xbb]).encode();
    encoded[12..20].copy_from_slice(&3_u64.to_be_bytes());

    assert_eq!(
        RecordEnvelope::decode(&encoded).unwrap_err(),
        EnvelopeError::PayloadTooShort {
            declared: 3,
            actual: 2,
        }
    );
}

#[test]
fn trailing_bytes_are_rejected() {
    let mut encoded = RecordEnvelope::new(1, 1, [0xaa]).encode();
    encoded.extend_from_slice(&[0xbb, 0xcc]);

    assert_eq!(
        RecordEnvelope::decode(&encoded).unwrap_err(),
        EnvelopeError::TrailingBytes {
            declared: 1,
            trailing: 2,
        }
    );
}
