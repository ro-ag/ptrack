#![deny(unsafe_code)]

mod discovery;
mod installer;
mod permissions;
mod signature;
mod staging;

pub use discovery::{
    Asset, Candidate, Client, Target, UpdateError, compare_versions, package_name, parse_version,
};
pub use installer::{ApplyAction, ApplyResult, Installer, recover_pending_apply};
pub use signature::{RELEASE_SIGNING_PUBLIC_KEY, SIGNATURE_ASSET_NAME};
pub use staging::{Progress, StageKind, StagedUpdate, discard_stage, load_stage, validate_stage};

#[cfg(test)]
mod discovery_test;
#[cfg(test)]
mod installer_test;
#[cfg(test)]
mod signature_test;
#[cfg(test)]
mod staging_test;
