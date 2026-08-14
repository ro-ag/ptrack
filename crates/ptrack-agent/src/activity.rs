use serde::{Deserialize, Serialize};

use crate::run::run_is_active;
use crate::{IntelligenceState, Run, RunIntelligence, RunState};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum ActivityState {
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "waiting")]
    Waiting,
    #[serde(rename = "blocked")]
    Blocked,
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "stale")]
    Stale,
    #[default]
    #[serde(rename = "unknown")]
    Unknown,
}

#[must_use]
pub fn derive_activity_state(run: &Run, intelligence: &RunIntelligence) -> ActivityState {
    if run.state == RunState::Stale {
        return ActivityState::Stale;
    }
    let active = run_is_active(run);
    match intelligence.state {
        IntelligenceState::Failed => ActivityState::Failed,
        IntelligenceState::Completed => ActivityState::Completed,
        IntelligenceState::Blocked if active => ActivityState::Blocked,
        IntelligenceState::Waiting if active => ActivityState::Waiting,
        _ if active => ActivityState::Running,
        _ => ActivityState::Unknown,
    }
}
