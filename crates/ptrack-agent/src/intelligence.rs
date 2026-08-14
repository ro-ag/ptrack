use serde::{Deserialize, Serialize};

use crate::run::run_is_active;
use crate::{EVENT_MODEL_VERSION, Event, EventKind, EventPhase, Run, Timestamp};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum IntelligenceState {
    #[default]
    #[serde(rename = "unknown")]
    Unknown,
    #[serde(rename = "working")]
    Working,
    #[serde(rename = "waiting")]
    Waiting,
    #[serde(rename = "blocked")]
    Blocked,
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "potentiallyDrifting")]
    PotentiallyDrifting,
}

impl IntelligenceState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Working => "working",
            Self::Waiting => "waiting",
            Self::Blocked => "blocked",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::PotentiallyDrifting => "potentiallyDrifting",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum IntelligenceConfidence {
    #[default]
    #[serde(rename = "")]
    Unset,
    #[serde(rename = "low")]
    Low,
    #[serde(rename = "medium")]
    Medium,
    #[serde(rename = "high")]
    High,
}

impl IntelligenceConfidence {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unset => "",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntelligenceEvidence {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub event_id: String,
    #[serde(default, skip_serializing_if = "is_unset_kind")]
    pub kind: EventKind,
    #[serde(default, skip_serializing_if = "is_unset_phase")]
    pub phase: EventPhase,
    #[serde(default, skip_serializing_if = "is_zero_timestamp")]
    pub observed_at: Timestamp,
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunIntelligence {
    pub run_id: String,
    pub state: IntelligenceState,
    pub confidence: IntelligenceConfidence,
    pub evidence: Vec<IntelligenceEvidence>,
    pub event_count: usize,
    #[serde(default, skip_serializing_if = "is_zero_timestamp")]
    pub last_event_at: Timestamp,
}

#[must_use]
pub fn derive_run_intelligence(run: &Run, events: &[Event]) -> RunIntelligence {
    let ordered = current_run_events(run, events);
    let mut result = RunIntelligence {
        run_id: run.id.clone(),
        state: IntelligenceState::Unknown,
        confidence: IntelligenceConfidence::Unset,
        evidence: Vec::new(),
        event_count: ordered.len(),
        last_event_at: ordered
            .last()
            .map_or(Timestamp::ZERO, |event| event.observed_at),
    };
    if run.exit.as_ref().is_some_and(|exit| exit.code != 0) {
        return with_evidence(
            result,
            IntelligenceState::Failed,
            IntelligenceConfidence::High,
            IntelligenceEvidence::reason("nonzero_process_exit"),
        );
    }
    let live = run_is_active(run);
    for event in ordered.iter().rev() {
        let mut evidence = IntelligenceEvidence::event(event, "explicit_event");
        let decision = if event.kind == EventKind::Lifecycle && event.phase == EventPhase::Failed {
            "explicit_lifecycle_failure".clone_into(&mut evidence.reason);
            Some((IntelligenceState::Failed, IntelligenceConfidence::High))
        } else if event.kind == EventKind::Lifecycle && event.phase == EventPhase::Completed {
            "explicit_lifecycle_completion".clone_into(&mut evidence.reason);
            Some((IntelligenceState::Completed, IntelligenceConfidence::High))
        } else if event.kind == EventKind::Error && fatal_event_class(&event.error_class) {
            "explicit_fatal_error".clone_into(&mut evidence.reason);
            Some((IntelligenceState::Failed, IntelligenceConfidence::High))
        } else if event.phase == EventPhase::Blocked && live {
            "explicit_blocked_event".clone_into(&mut evidence.reason);
            Some((IntelligenceState::Blocked, IntelligenceConfidence::Medium))
        } else if event.phase == EventPhase::Waiting && live {
            "explicit_waiting_event".clone_into(&mut evidence.reason);
            Some((IntelligenceState::Waiting, IntelligenceConfidence::Medium))
        } else if live
            && drift_event_class(&event.error_class)
            && event_correlation_is_current(run, event)
        {
            "explicit_scope_mismatch".clone_into(&mut evidence.reason);
            Some((
                IntelligenceState::PotentiallyDrifting,
                IntelligenceConfidence::Medium,
            ))
        } else if live
            && matches!(
                event.phase,
                EventPhase::Started
                    | EventPhase::Progress
                    | EventPhase::Completed
                    | EventPhase::Failed
            )
        {
            if event.phase == EventPhase::Failed {
                "operation_failure_while_run_live"
            } else {
                "recent_observable_activity"
            }
            .clone_into(&mut evidence.reason);
            Some((IntelligenceState::Working, IntelligenceConfidence::Medium))
        } else {
            None
        };
        if let Some((state, confidence)) = decision {
            return with_evidence(result, state, confidence, evidence);
        }
    }
    if live {
        result = with_evidence(
            result,
            IntelligenceState::Working,
            IntelligenceConfidence::Low,
            IntelligenceEvidence::reason("live_run_without_structured_progress"),
        );
    }
    result
}

pub(crate) fn current_run_events(run: &Run, events: &[Event]) -> Vec<Event> {
    let mut ordered: Vec<Event> = events
        .iter()
        .filter(|event| {
            event.model_version == EVENT_MODEL_VERSION
                && event.run_id == run.id
                && event.provider == run.provider
                && event.host_sequence != 0
                && event.lifecycle_revision == run.lifecycle_revision
        })
        .cloned()
        .collect();
    ordered.sort_by(|left, right| {
        left.host_sequence
            .cmp(&right.host_sequence)
            .then(left.observed_at.cmp(&right.observed_at))
    });
    ordered
}

impl IntelligenceEvidence {
    fn reason(reason: &str) -> Self {
        Self {
            event_id: String::new(),
            kind: EventKind::Unset,
            phase: EventPhase::Unset,
            observed_at: Timestamp::ZERO,
            reason: reason.to_owned(),
        }
    }

    fn event(event: &Event, reason: &str) -> Self {
        Self {
            event_id: event.id.clone(),
            kind: event.kind,
            phase: event.phase,
            observed_at: event.observed_at,
            reason: reason.to_owned(),
        }
    }
}

fn with_evidence(
    mut result: RunIntelligence,
    state: IntelligenceState,
    confidence: IntelligenceConfidence,
    evidence: IntelligenceEvidence,
) -> RunIntelligence {
    result.state = state;
    result.confidence = confidence;
    result.evidence = vec![evidence];
    result
}

fn fatal_event_class(value: &str) -> bool {
    matches!(
        value,
        "fatal" | "fatal_error" | "session_failure" | "process_failure"
    )
}

fn drift_event_class(value: &str) -> bool {
    matches!(
        value,
        "scope_mismatch" | "task_mismatch" | "repository_mismatch"
    )
}

fn event_correlation_is_current(run: &Run, event: &Event) -> bool {
    let Some(current) = run.association.as_ref() else {
        return false;
    };
    let correlation = &event.correlation;
    correlation.task_id != 0
        && current.project_root == run.project_root
        && current.live_id == run.id
        && correlation.project_root == run.project_root
        && correlation.terminal_id == run.terminal_id
        && correlation.plan_id == current.target.plan_id
        && correlation.task_id == current.target.task_id
        && correlation.generation == current.generation
        && correlation.association_revision == current.revision
}

const fn is_unset_kind(value: &EventKind) -> bool {
    matches!(value, EventKind::Unset)
}
const fn is_unset_phase(value: &EventPhase) -> bool {
    matches!(value, EventPhase::Unset)
}
const fn is_zero_timestamp(value: &Timestamp) -> bool {
    value.is_zero()
}
