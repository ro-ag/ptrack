use super::{
    ActivityState, IntelligenceConfidence, IntelligenceState, LeaseState, ProcessState,
    RegistrationKind, Run, RunIntelligence, RunState, derive_activity_state,
};

fn intelligence(state: IntelligenceState) -> RunIntelligence {
    RunIntelligence {
        run_id: "run-1".to_owned(),
        state,
        confidence: IntelligenceConfidence::Medium,
        evidence: Vec::new(),
        event_count: 0,
        last_event_at: super::Timestamp::ZERO,
    }
}

#[test]
fn activity_mapping_is_conservative() {
    let active = Run {
        registration_kind: RegistrationKind::Launched,
        state: RunState::Running,
        process_state: ProcessState::Running,
        ..Run::default()
    };
    for (state, expected) in [
        (IntelligenceState::Unknown, ActivityState::Running),
        (IntelligenceState::Waiting, ActivityState::Waiting),
        (IntelligenceState::Blocked, ActivityState::Blocked),
        (IntelligenceState::Completed, ActivityState::Completed),
        (IntelligenceState::Failed, ActivityState::Failed),
    ] {
        assert_eq!(
            derive_activity_state(&active, &intelligence(state)),
            expected
        );
    }
    let inactive = Run {
        registration_kind: RegistrationKind::External,
        state: RunState::Exited,
        lease_state: LeaseState::Expired,
        ..Run::default()
    };
    assert_eq!(
        derive_activity_state(&inactive, &intelligence(IntelligenceState::Waiting)),
        ActivityState::Unknown
    );
    let stale = Run {
        state: RunState::Stale,
        ..active
    };
    assert_eq!(
        derive_activity_state(&stale, &intelligence(IntelligenceState::Failed)),
        ActivityState::Stale
    );
}
