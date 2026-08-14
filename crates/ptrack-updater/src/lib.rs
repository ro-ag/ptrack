#![deny(unsafe_code)]

mod discovery;
mod installer;
mod permissions;
mod staging;

pub use discovery::{
    Asset, Candidate, Client, Target, UpdateError, compare_versions, package_name, parse_version,
};
pub use installer::{ApplyAction, ApplyResult, Installer, recover_pending_apply};
pub use staging::{Progress, StageKind, StagedUpdate, discard_stage, load_stage, validate_stage};

#[cfg(test)]
mod discovery_test;
#[cfg(test)]
mod installer_test;
#[cfg(test)]
mod staging_test;
