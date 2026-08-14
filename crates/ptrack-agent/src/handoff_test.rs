use super::{
    Association, AssociationTarget, Event, EventCorrelation, EventKind, EventPhase, ProcessState,
    RegistrationKind, Run, RunState, Timestamp, build_handoff_preview,
};

fn run() -> Run {
    Run {
        id: "run-1".to_owned(),
        provider: "codex".to_owned(),
        project_root: "/project".to_owned(),
        terminal_id: "terminal-1".to_owned(),
        registration_kind: RegistrationKind::Launched,
        state: RunState::Running,
        process_state: ProcessState::Running,
        lifecycle_revision: 1,
        association: Some(Association {
            version: 1,
            project_root: "/project".to_owned(),
            generation: 3,
            live_id: "run-1".to_owned(),
            target: AssociationTarget {
                plan_id: 2,
                task_id: 9,
            },
            revision: 4,
        }),
        ..Run::default()
    }
}

fn event(sequence: u64, kind: EventKind, phase: EventPhase) -> Event {
    Event {
        model_version: 1,
        id: format!("event-{sequence}"),
        run_id: "run-1".to_owned(),
        provider: "codex".to_owned(),
        host_sequence: sequence,
        lifecycle_revision: 1,
        kind,
        phase,
        observed_at: Timestamp::from_unix_nanoseconds(i128::from(sequence) * 1_000_000_000),
        correlation: EventCorrelation {
            project_root: "/project".to_owned(),
            terminal_id: "terminal-1".to_owned(),
            plan_id: 2,
            task_id: 9,
            generation: 3,
            association_revision: 4,
            ..EventCorrelation::default()
        },
        ..Event::default()
    }
}

#[test]
fn preview_uses_bounded_newest_structured_evidence() {
    let mut file = event(1, EventKind::File, EventPhase::Completed);
    file.paths = vec!["src/lib.rs".to_owned(), "src/../tests/a.rs".to_owned()];
    let mut test = event(2, EventKind::Test, EventPhase::Completed);
    test.subject = "cargo-test".to_owned();
    let value = build_handoff_preview(&run(), &[file, test]);
    assert_eq!(value.included_event_ids, ["event-2", "event-1"]);
    assert_eq!(value.considered_events, 2);
    assert_eq!(
        value.text,
        "Agent run state: working (medium confidence).\nContext: plan #2, task #9.\n- Test completed: cargo-test.\n- File activity completed: src/lib.rs, tests/a.rs."
    );
    assert_eq!(
        serde_json::to_value(&value)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>()
            .len(),
        4
    );
}

#[test]
fn preview_excludes_stale_correlation_and_legacy_private_content() {
    let mut stale = event(1, EventKind::Tool, EventPhase::Completed);
    stale.correlation.revision_for_test(3);
    let mut reasoning = event(2, EventKind::Summary, EventPhase::Completed);
    reasoning.summary = "<analysis>private steps</analysis>".to_owned();
    let mut credential = event(3, EventKind::Summary, EventPhase::Completed);
    credential.summary = "-----BEGIN PRIVATE KEY-----".to_owned();
    let value = build_handoff_preview(&run(), &[stale, reasoning, credential]);
    assert!(value.included_event_ids.is_empty());
    assert!(
        value
            .text
            .ends_with("No retained structured work-product events for the current context.")
    );
}

#[test]
fn preview_reredacts_legacy_bearer_and_assignment_summaries() {
    let mut summary = event(1, EventKind::Summary, EventPhase::Completed);
    summary.summary = "Bearer TOP_SECRET token=SECOND_SECRET".to_owned();
    let value = build_handoff_preview(&run(), &[summary]);
    assert_eq!(value.included_event_ids, ["event-1"]);
    assert!(value.text.contains("Bearer [redacted] token=[redacted]"));
    assert!(!value.text.contains("TOP_SECRET") && !value.text.contains("SECOND_SECRET"));
}

#[test]
fn preview_drops_nbsp_separated_legacy_assigned_scalar_value() {
    let mut tool = event(1, EventKind::Tool, EventPhase::Completed);
    tool.subject = "token=\u{00a0}LEGACY_SECRET".to_owned();
    let value = build_handoff_preview(&run(), &[tool]);
    assert_eq!(value.included_event_ids, ["event-1"]);
    assert!(value.text.contains("- Tool completed."));
    assert!(!value.text.contains("LEGACY_SECRET") && !value.text.contains('\u{00a0}'));
}

#[test]
fn preview_applies_event_and_utf8_byte_limits() {
    let mut events = Vec::new();
    for sequence in 1..=10 {
        let mut value = event(sequence, EventKind::Tool, EventPhase::Progress);
        value.subject = format!("item{sequence}-{}", "界".repeat(160));
        events.push(value);
    }
    let value = build_handoff_preview(&run(), &events);
    assert_eq!(value.included_event_ids.len(), 8);
    assert!(value.truncated);
    assert!(value.text.len() <= 2048);
    assert!(value.text.ends_with("\n…"));
    assert!(std::str::from_utf8(value.text.as_bytes()).is_ok());
}

trait CorrelationTestExt {
    fn revision_for_test(&mut self, revision: u64);
}

impl CorrelationTestExt for EventCorrelation {
    fn revision_for_test(&mut self, revision: u64) {
        self.association_revision = revision;
    }
}
