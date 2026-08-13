use super::{
    EVENT_MODEL_VERSION, Event, EventCorrelation, EventKind, EventNotificationKind,
    EventObservation, EventOutcome, EventPhase, Timestamp,
};

#[test]
fn event_json_round_trip_preserves_only_structured_evidence() {
    let value = Event {
        model_version: EVENT_MODEL_VERSION,
        id: "host-event-1".to_owned(),
        run_id: "run-1".to_owned(),
        provider: "codex".to_owned(),
        source_id: "provider-event-9".to_owned(),
        source_sequence: 9,
        host_sequence: 4,
        lifecycle_revision: 2,
        kind: EventKind::Test,
        phase: EventPhase::Failed,
        outcome: EventOutcome::Failed,
        subject: "cargo-test".to_owned(),
        paths: vec!["src/event.rs".to_owned()],
        commit_sha: "0123456789abcdef".to_owned(),
        exit_code: Some(1),
        error_class: "test_failure".to_owned(),
        summary: "bounded".to_owned(),
        occurred_at: Timestamp::from_unix_nanoseconds(2_000_000_000),
        observed_at: Timestamp::from_unix_nanoseconds(3_000_000_000),
        correlation: EventCorrelation {
            project_root: "/project".to_owned(),
            ..EventCorrelation::default()
        },
        notification: EventNotificationKind::Unset,
    };
    let encoded = serde_json::to_string(&value).unwrap();
    assert_eq!(serde_json::from_str::<Event>(&encoded).unwrap(), value);
    assert_eq!(
        encoded,
        r#"{"modelVersion":1,"id":"host-event-1","runId":"run-1","provider":"codex","sourceId":"provider-event-9","sourceSequence":9,"hostSequence":4,"lifecycleRevision":2,"kind":"test","phase":"failed","outcome":"failed","subject":"cargo-test","paths":["src/event.rs"],"commitSha":"0123456789abcdef","exitCode":1,"errorClass":"test_failure","summary":"bounded","occurredAt":"1970-01-01T00:00:02Z","observedAt":"1970-01-01T00:00:03Z","correlation":{"projectRoot":"/project"}}"#
    );
}

#[test]
fn event_observation_cannot_deserialize_adapter_recognition() {
    let decoded: EventObservation = serde_json::from_str(r#"{"modelVersion":1,"sourceId":"x","sourceSequence":1,"kind":"lifecycle","phase":"waiting","notification":"question","recognizedNotification":true}"#).unwrap();
    assert!(!decoded.recognized_notification);
    assert_eq!(decoded.notification, EventNotificationKind::Question);
}

#[test]
fn event_contract_enums_are_closed() {
    assert!(EventKind::Lifecycle.is_valid());
    assert!(EventPhase::Completed.is_valid());
    assert!(EventOutcome::Succeeded.is_valid());
    assert!(EventNotificationKind::Completion.is_valid());
    assert!(serde_json::from_str::<EventKind>(r#""prompt""#).is_err());
    assert!(serde_json::from_str::<EventPhase>(r#""idle""#).is_err());
}
