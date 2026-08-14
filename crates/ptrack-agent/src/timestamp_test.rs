use super::Timestamp;

#[test]
fn timestamp_matches_go_zero_and_rfc3339_nano_json() {
    assert_eq!(
        serde_json::to_string(&Timestamp::ZERO).unwrap(),
        r#""0001-01-01T00:00:00Z""#
    );
    let value = Timestamp::from_unix_nanoseconds(1_723_314_000_123_456_789);
    let encoded = serde_json::to_string(&value).unwrap();
    assert_eq!(serde_json::from_str::<Timestamp>(&encoded).unwrap(), value);
    assert!(encoded.ends_with("Z\""));
}
