#![allow(clippy::unicode_not_nfc)] // Intentional Go Unicode-folding canaries.

use super::{
    EVENT_MODEL_VERSION, Event, EventKind, EventNotificationKind, EventObservation, EventOutcome,
    EventPhase, EventPrivacyError, Timestamp, default_event_privacy_policy,
    normalize_event_observation, normalize_provider_event, retain_events,
};
use crate::{PROVIDER_EVENT_MODEL_VERSION, ProviderEvent};

fn observation() -> EventObservation {
    EventObservation {
        model_version: EVENT_MODEL_VERSION,
        source_id: "file-1".to_owned(),
        source_sequence: 1,
        kind: EventKind::File,
        phase: EventPhase::Progress,
        subject: "write".to_owned(),
        paths: vec![
            "internal/other.rs".to_owned(),
            "internal/../internal/event.rs".to_owned(),
            "internal/event.rs".to_owned(),
        ],
        ..EventObservation::default()
    }
}

#[test]
fn normalization_bounds_paths_and_rejects_authority_shaped_content() {
    let now = Timestamp::from_unix_nanoseconds(1_800_000_000_000_000_000);
    let value = normalize_event_observation(
        "/project",
        now,
        default_event_privacy_policy(),
        observation(),
    )
    .unwrap();
    assert_eq!(value.paths, ["internal/event.rs", "internal/other.rs"]);
    for invalid in [
        {
            let mut value = observation();
            value.model_version = 2;
            value
        },
        {
            let mut value = observation();
            value.source_sequence = 0;
            value
        },
        {
            let mut value = observation();
            value.paths = vec!["../secret".to_owned()];
            value
        },
        {
            let mut value = observation();
            value.subject = "token=SECRET".to_owned();
            value
        },
        {
            let mut value = observation();
            value.source_id = "sk-abcdefghijklmnopqrstuv".to_owned();
            value
        },
        {
            let mut value = observation();
            value.commit_sha = "not-a-sha".to_owned();
            value
        },
        {
            let mut value = observation();
            value.paths = vec!["token=\u{00a0}PATH_SECRET".to_owned()];
            value
        },
    ] {
        assert!(
            normalize_event_observation("/project", now, default_event_privacy_policy(), invalid)
                .is_err()
        );
    }
}

#[test]
fn summaries_are_explicit_redacted_and_reasoning_free() {
    let now = Timestamp::from_unix_nanoseconds(1_800_000_000_000_000_000);
    let mut policy = default_event_privacy_policy();
    policy.allow_summaries = true;
    let mut value = observation();
    value.kind = EventKind::Summary;
    value.phase = EventPhase::Completed;
    value.paths.clear();
    value.summary = "Bearer TOP_SECRET token=SECOND_SECRET https://user:pass@example.com/path?api_key=THIRD_SECRET".to_owned();
    let value = normalize_event_observation("/project", now, policy, value).unwrap();
    assert_eq!(
        value.summary,
        "Bearer [redacted] token=[redacted] https://example.com/path?redacted]"
    );
    for content in [
        "<thinking>private</thinking>",
        "Chain-of-thought: private",
        "-----BEGIN PRIVATE KEY-----",
    ] {
        let mut invalid = observation();
        invalid.kind = EventKind::Summary;
        invalid.phase = EventPhase::Completed;
        invalid.paths.clear();
        invalid.summary = content.to_owned();
        let error = normalize_event_observation("/project", now, policy, invalid)
            .unwrap_err()
            .to_string();
        assert!(!error.contains(content));
    }
}

#[test]
fn credential_boundaries_and_case_folding_match_go_regexp_and_strings() {
    let now = Timestamp::from_unix_nanoseconds(1_800_000_000_000_000_000);
    let mut policy = default_event_privacy_policy();
    policy.allow_summaries = true;

    let mut boundary = observation();
    boundary.kind = EventKind::Summary;
    boundary.phase = EventPhase::Completed;
    boundary.paths.clear();
    boundary.summary = "界Bearer TOP_SECRET 界token=SECOND_SECRET".to_owned();
    let normalized = normalize_event_observation("/project", now, policy, boundary).unwrap();
    assert_eq!(normalized.summary, "界Bearer [redacted] 界token=[redacted]");

    for rejected in [
        "界sk-abcdefghijklmnop界",
        "<THINKING>private steps</THINKING>",
        "-----BEGIN PRIVATE KEY-----",
    ] {
        let mut value = observation();
        value.kind = EventKind::Summary;
        value.phase = EventPhase::Completed;
        value.paths.clear();
        value.summary = rejected.to_owned();
        assert!(normalize_event_observation("/project", now, policy, value).is_err());
    }
}

#[test]
fn url_redaction_preserves_go_empty_path_serialization() {
    let now = Timestamp::from_unix_nanoseconds(1_800_000_000_000_000_000);
    let mut policy = default_event_privacy_policy();
    policy.allow_summaries = true;
    let mut value = observation();
    value.kind = EventKind::Summary;
    value.phase = EventPhase::Completed;
    value.paths.clear();
    value.summary =
        "https://user:pass@example.com https://example.com?foo=x https://example.com/#fragment"
            .to_owned();
    let normalized = normalize_event_observation("/project", now, policy, value).unwrap();
    assert_eq!(
        normalized.summary,
        "https://example.com https://example.com?redacted https://example.com/"
    );
}

#[test]
fn url_redaction_matches_bounded_go_net_url_differentials() {
    let now = Timestamp::from_unix_nanoseconds(1_800_000_000_000_000_000);
    let mut policy = default_event_privacy_policy();
    policy.allow_summaries = true;
    for (raw, expected) in [
        (
            "https://例え.テスト",
            "https://%E4%BE%8B%E3%81%88.%E3%83%86%E3%82%B9%E3%83%88",
        ),
        ("https://example.com/界", "https://example.com/%E7%95%8C"),
        ("https://example.com/a%2fb", "https://example.com/a%2fb"),
        ("https://example.com/%zz", "[redacted-url]"),
        ("https://exa%mple.com/path", "[redacted-url]"),
        ("https://example.com?", "https://example.com?"),
        ("https://example.com/path).", "https://example.com/path)."),
        (
            "https://user:pass@例え.テスト/a%2fb?foo=x#fragment",
            "https://%E4%BE%8B%E3%81%88.%E3%83%86%E3%82%B9%E3%83%88/a%2fb?redacted",
        ),
        (
            "https://example.com/path?foo=%zz",
            "https://example.com/path?redacted",
        ),
        ("https://[::1]/a", "https://[::1]/a"),
        ("https://[::1]:443/a", "https://[::1]:443/a"),
        ("https://[fe80::1%25en0]/a", "https://[fe80::1%25en0]/a"),
        ("https://[fe80::1%25en%32]/a", "https://[fe80::1%25en2]/a"),
        ("https://[fe80::1%25a%20b]/a", "https://[fe80::1%25a%20b]/a"),
        (
            "https://[::ffff:192.0.2.1]/a",
            "https://[::ffff:192.0.2.1]/a",
        ),
        ("https://[]/a", "[redacted-url]"),
        ("https://[127.0.0.1]/a", "[redacted-url]"),
        ("https://[not-ip]/a", "[redacted-url]"),
        ("https://[fe80::1%25]/a", "[redacted-url]"),
        ("https://[fe80::1%25a%2fb]/a", "[redacted-url]"),
        ("https://[fe80::1%en0]/a", "[redacted-url]"),
        ("https://[::1]x/a", "[redacted-url]"),
        ("https://[::1]:x/a", "[redacted-url]"),
    ] {
        let mut value = observation();
        value.kind = EventKind::Summary;
        value.phase = EventPhase::Completed;
        value.paths.clear();
        value.summary = raw.to_owned();
        let normalized = normalize_event_observation("/project", now, policy, value).unwrap();
        assert_eq!(normalized.summary, expected, "raw URL {raw:?}");
    }
}

#[test]
fn only_adapter_recognized_notifications_cross_privacy_boundary() {
    let now = Timestamp::from_unix_nanoseconds(1_800_000_000_000_000_000);
    let provider = ProviderEvent {
        model_version: PROVIDER_EVENT_MODEL_VERSION,
        id: "notice-1".to_owned(),
        sequence: 1,
        event_type: "question".to_owned(),
        ..ProviderEvent::default()
    };
    let trusted = normalize_provider_event("codex", provider).unwrap();
    let normalized =
        normalize_event_observation("/project", now, default_event_privacy_policy(), trusted)
            .unwrap();
    assert_eq!(normalized.notification, EventNotificationKind::Question);
    let direct: EventObservation = serde_json::from_str(r#"{"modelVersion":1,"sourceId":"notice-2","sourceSequence":2,"kind":"lifecycle","phase":"waiting","notification":"question"}"#).unwrap();
    assert_eq!(
        normalize_event_observation("/project", now, default_event_privacy_policy(), direct)
            .unwrap_err()
            .to_string(),
        "unsupported agent event notification"
    );
}

#[test]
fn retention_applies_age_count_order_and_disabled_erasure() {
    let now = Timestamp::from_unix_nanoseconds(1_800_000_000_000_000_000);
    let mut policy = default_event_privacy_policy();
    policy.retain_last = 2;
    policy.retain_for = time::Duration::hours(1);
    let events = [
        Event {
            id: "newest".to_owned(),
            host_sequence: 4,
            observed_at: now,
            ..Event::default()
        },
        Event {
            id: "expired".to_owned(),
            host_sequence: 1,
            observed_at: now.add_seconds(-7200),
            ..Event::default()
        },
        Event {
            id: "older".to_owned(),
            host_sequence: 2,
            observed_at: now.add_seconds(-120),
            ..Event::default()
        },
        Event {
            id: "newer".to_owned(),
            host_sequence: 3,
            observed_at: now.add_seconds(-60),
            ..Event::default()
        },
    ];
    let retained = retain_events(&events, now, policy).unwrap();
    assert_eq!(
        retained
            .iter()
            .map(|event| event.id.as_str())
            .collect::<Vec<_>>(),
        ["newer", "newest"]
    );
    policy.collection_enabled = false;
    assert!(retain_events(&events, now, policy).unwrap().is_empty());
    assert_eq!(
        normalize_event_observation("/project", now, policy, observation()).unwrap_err(),
        EventPrivacyError::CollectionDisabled
    );
}

#[test]
fn kind_scoped_fields_and_notifications_are_closed() {
    let now = Timestamp::from_unix_nanoseconds(1_800_000_000_000_000_000);
    let mut value = observation();
    value.commit_sha = "01234567".to_owned();
    assert!(
        normalize_event_observation("/project", now, default_event_privacy_policy(), value)
            .is_err()
    );
    let mut notice = observation();
    notice.kind = EventKind::Lifecycle;
    notice.phase = EventPhase::Completed;
    notice.paths.clear();
    notice.subject.clear();
    notice.outcome = EventOutcome::Succeeded;
    notice.notification = EventNotificationKind::Completion;
    assert!(
        normalize_event_observation("/project", now, default_event_privacy_policy(), notice)
            .is_err()
    );
}
