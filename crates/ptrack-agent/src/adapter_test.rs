use super::{
    EventKind, EventNotificationKind, EventOutcome, EventPhase, PROVIDER_EVENT_MODEL_VERSION,
    ProviderEvent, normalize_provider_event, supported_event_providers,
};

fn provider_event(event_type: &str) -> ProviderEvent {
    ProviderEvent {
        model_version: PROVIDER_EVENT_MODEL_VERSION,
        id: "event-1".to_owned(),
        sequence: 1,
        event_type: event_type.to_owned(),
        ..ProviderEvent::default()
    }
}

#[test]
fn adapters_cover_profiles_and_reserve_completion() {
    assert_eq!(
        supported_event_providers(),
        ["agy", "claude", "codex", "gemini", "kimi", "opencode"]
    );
    for (provider, event_type, kind, phase) in [
        (
            "codex",
            "turn.started",
            EventKind::Lifecycle,
            EventPhase::Progress,
        ),
        ("claude", "PreToolUse", EventKind::Tool, EventPhase::Started),
        (
            "gemini",
            "AfterTool",
            EventKind::Tool,
            EventPhase::Completed,
        ),
        (
            "agy",
            "session.failed",
            EventKind::Lifecycle,
            EventPhase::Failed,
        ),
        (
            "kimi",
            "PostToolUseFailure",
            EventKind::Tool,
            EventPhase::Failed,
        ),
        (
            "opencode",
            "file.edited",
            EventKind::File,
            EventPhase::Completed,
        ),
    ] {
        let value = normalize_provider_event(provider, provider_event(event_type)).unwrap();
        assert_eq!((value.kind, value.phase), (kind, phase));
    }
    assert_eq!(
        normalize_provider_event("codex", provider_event("turn.completed"))
            .unwrap()
            .phase,
        EventPhase::Waiting
    );
    assert_eq!(
        normalize_provider_event("codex", provider_event("sessionend"))
            .unwrap()
            .phase,
        EventPhase::Completed
    );
    assert_eq!(
        normalize_provider_event("kimi", provider_event("SessionEnd"))
            .unwrap()
            .notification,
        EventNotificationKind::Completion
    );
}

#[test]
fn kimi_adapter_maps_current_hook_lifecycle() {
    for (event_type, kind, phase, notification) in [
        (
            "SessionStart",
            EventKind::Lifecycle,
            EventPhase::Started,
            EventNotificationKind::Unset,
        ),
        (
            "TurnStarted",
            EventKind::Lifecycle,
            EventPhase::Progress,
            EventNotificationKind::Unset,
        ),
        (
            "PermissionRequest",
            EventKind::Lifecycle,
            EventPhase::Waiting,
            EventNotificationKind::ApprovalRequested,
        ),
        (
            "StopFailure",
            EventKind::Error,
            EventPhase::Failed,
            EventNotificationKind::Failure,
        ),
    ] {
        let value = normalize_provider_event("kimi", provider_event(event_type)).unwrap();
        assert_eq!(
            (value.kind, value.phase, value.notification),
            (kind, phase, notification)
        );
    }
}

#[test]
fn adapters_strip_notifications_and_fail_closed() {
    let mut input = provider_event("PermissionRequest");
    input.subject = "QUESTION_CANARY".to_owned();
    input.paths = vec!["SECRET_CANARY".to_owned()];
    input.summary = "PROMPT_CANARY".to_owned();
    let value = normalize_provider_event("codex", input).unwrap();
    assert_eq!(value.notification, EventNotificationKind::ApprovalRequested);
    assert!(value.recognized_notification);
    assert!(value.subject.is_empty() && value.paths.is_empty() && value.summary.is_empty());
    assert!(normalize_provider_event("codex", provider_event("summary.completed")).is_err());
    assert!(normalize_provider_event("future", provider_event("tool.started")).is_err());
    assert!(normalize_provider_event("future", provider_event("lifecycle.progress")).is_ok());
}

#[test]
fn codex_items_and_nonzero_exits_map_exactly() {
    let mut item = provider_event("item.completed");
    item.category = EventKind::Test;
    let item = normalize_provider_event("codex", item).unwrap();
    assert_eq!(
        (item.kind, item.phase, item.outcome),
        (
            EventKind::Test,
            EventPhase::Completed,
            EventOutcome::Succeeded
        )
    );
    let mut ended = provider_event("sessionend");
    ended.exit_code = Some(2);
    let ended = normalize_provider_event("codex", ended).unwrap();
    assert_eq!(
        (ended.phase, ended.outcome, ended.notification),
        (
            EventPhase::Failed,
            EventOutcome::Failed,
            EventNotificationKind::Failure
        )
    );
}
