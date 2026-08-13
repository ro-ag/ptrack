use serde::{Deserialize, Serialize};

use crate::{Association, Timestamp};

macro_rules! string_enum {
    ($name:ident { $($variant:ident => $value:literal),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
        pub enum $name {
            #[default]
            #[serde(rename = "")]
            Unset,
            $(#[serde(rename = $value)] $variant),+
        }
        impl $name {
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self { Self::Unset => "", $(Self::$variant => $value),+ }
            }
        }
    };
}

string_enum!(RegistrationKind {
    Launched => "launched",
    External => "external",
});
string_enum!(RunState {
    Running => "running",
    Exited => "exited",
    Stale => "stale",
    Unknown => "unknown",
});
string_enum!(ProcessState {
    Running => "running",
    Exited => "exited",
    Unknown => "unknown",
});
string_enum!(LeaseState {
    None => "none",
    Active => "active",
    Expired => "expired",
});

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Exit {
    pub code: i32,
    pub result: String,
    pub occurred_at: Timestamp,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Run {
    pub id: String,
    pub profile: String,
    pub provider: String,
    pub pid: i32,
    pub process_state: ProcessState,
    pub lease_state: LeaseState,
    pub project_root: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub association: Option<Association>,
    pub terminal_id: String,
    pub cwd: String,
    pub started_at: Timestamp,
    pub last_activity_at: Timestamp,
    pub last_heartbeat_at: Timestamp,
    pub state: RunState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<Exit>,
    #[serde(rename = "registrationKind")]
    pub registration_kind: RegistrationKind,
    #[serde(skip)]
    pub lifecycle_revision: u64,
}

impl Default for Run {
    fn default() -> Self {
        Self {
            id: String::new(),
            profile: String::new(),
            provider: String::new(),
            pid: 0,
            process_state: ProcessState::Unset,
            lease_state: LeaseState::Unset,
            project_root: String::new(),
            association: None,
            terminal_id: String::new(),
            cwd: String::new(),
            started_at: Timestamp::ZERO,
            last_activity_at: Timestamp::ZERO,
            last_heartbeat_at: Timestamp::ZERO,
            state: RunState::Unset,
            exit: None,
            registration_kind: RegistrationKind::Unset,
            lifecycle_revision: 0,
        }
    }
}

#[must_use]
pub(crate) fn run_is_active(run: &Run) -> bool {
    match run.registration_kind {
        RegistrationKind::Launched => {
            run.state == RunState::Running && run.process_state == ProcessState::Running
        }
        RegistrationKind::External => {
            run.state == RunState::Running && run.lease_state == LeaseState::Active
        }
        RegistrationKind::Unset => false,
    }
}
