use super::{
    Association, AssociationTarget, Event, EventCorrelation, EventKind, EventPhase, Exit,
    IntelligenceConfidence, IntelligenceState, ProcessState, RegistrationKind, Run, RunState,
    Timestamp, derive_run_intelligence,
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
        lifecycle_revision: 2,
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
        lifecycle_revision: 2,
        kind,
        phase,
        observed_at: Timestamp::from_unix_nanoseconds(i128::from(sequence) * 1_000_000_000),
        correlation: EventCorrelation {
            project_root: "/project".to_owned(),
            terminal_id: "terminal-1".to_owned(),
            ..EventCorrelation::default()
        },
        ..Event::default()
    }
}

#[test]
fn derivation_uses_newest_current_explicit_evidence() {
    let run = run();
    let waiting = event(2, EventKind::Lifecycle, EventPhase::Waiting);
    let progress = event(3, EventKind::Tool, EventPhase::Progress);
    let stale = Event {
        id: "stale".to_owned(),
        lifecycle_revision: 1,
        phase: EventPhase::Failed,
        ..event(99, EventKind::Lifecycle, EventPhase::Failed)
    };
    let value = derive_run_intelligence(&run, &[progress, stale, waiting]);
    assert_eq!(
        (value.state, value.confidence, value.event_count),
        (
            IntelligenceState::Working,
            IntelligenceConfidence::Medium,
            2
        )
    );
    assert_eq!(value.evidence[0].reason, "recent_observable_activity");
    assert_eq!(value.evidence[0].event_id, "event-3");
}

#[test]
fn failure_completion_and_successful_exit_precedence_is_conservative() {
    let mut run = run();
    run.exit = Some(Exit {
        code: 7,
        result: "failed".to_owned(),
        occurred_at: Timestamp::ZERO,
    });
    assert_eq!(
        derive_run_intelligence(&run, &[]).state,
        IntelligenceState::Failed
    );
    run.exit.as_mut().unwrap().code = 0;
    run.state = RunState::Exited;
    run.process_state = ProcessState::Exited;
    assert_eq!(
        derive_run_intelligence(&run, &[]).state,
        IntelligenceState::Unknown
    );
    assert_eq!(
        derive_run_intelligence(
            &run,
            &[event(1, EventKind::Lifecycle, EventPhase::Completed)]
        )
        .state,
        IntelligenceState::Completed
    );
}

#[test]
fn drift_requires_full_current_association_correlation() {
    let mut run = run();
    run.association = Some(Association {
        version: 1,
        project_root: "/project".to_owned(),
        generation: 5,
        live_id: "run-1".to_owned(),
        target: AssociationTarget {
            plan_id: 2,
            task_id: 9,
        },
        revision: 4,
    });
    let mut mismatch = event(1, EventKind::Error, EventPhase::Progress);
    mismatch.error_class = "scope_mismatch".to_owned();
    assert_eq!(
        derive_run_intelligence(&run, &[mismatch.clone()]).state,
        IntelligenceState::Working
    );
    mismatch.correlation.plan_id = 2;
    mismatch.correlation.task_id = 9;
    mismatch.correlation.generation = 5;
    mismatch.correlation.association_revision = 4;
    assert_eq!(
        derive_run_intelligence(&run, &[mismatch]).state,
        IntelligenceState::PotentiallyDrifting
    );
}

#[test]
fn intelligence_json_exposes_metadata_only_evidence() {
    let value = derive_run_intelligence(
        &run(),
        &[event(1, EventKind::Lifecycle, EventPhase::Waiting)],
    );
    assert_eq!(
        serde_json::to_string(&value).unwrap(),
        r#"{"runId":"run-1","state":"waiting","confidence":"medium","evidence":[{"eventId":"event-1","kind":"lifecycle","phase":"waiting","observedAt":"1970-01-01T00:00:01Z","reason":"explicit_waiting_event"}],"eventCount":1,"lastEventAt":"1970-01-01T00:00:01Z"}"#
    );
}
