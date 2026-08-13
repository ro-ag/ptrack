//! Generation-scoped agent runtime and coordination for p-track.
//!
//! It owns bounded registration, private history, a loopback-only integration
//! endpoint, structured evidence, and memory-only coordination. It grants no
//! capability authority and workflow approval never executes a command.

#![allow(clippy::trivially_copy_pass_by_ref)] // Serde skip callbacks take references.

mod activity;
mod adapter;
mod association;
mod coordination;
mod correlation;
mod event;
mod handoff;
mod integration;
mod intelligence;
mod launch_context;
mod persistence;
mod privacy;
mod process;
mod registry;
mod run;
mod timestamp;

pub use activity::{ActivityState, derive_activity_state};
pub use adapter::{
    PROVIDER_EVENT_MODEL_VERSION, ProviderEvent, normalize_provider_event,
    supported_event_providers,
};
pub use association::{
    ASSOCIATION_VERSION_V1, Association, AssociationCatalog, AssociationError, AssociationHost,
    AssociationPointer, AssociationTarget, association_generation, association_project_root,
    bind_association,
};
pub use coordination::*;
pub use correlation::{EventCorrelation, discover_repository_root, event_correlation_for_run};
pub use event::{
    EVENT_MODEL_VERSION, Event, EventKind, EventNotificationKind, EventObservation, EventOutcome,
    EventPhase,
};
pub use handoff::{HandoffPreview, build_handoff_preview};
pub use integration::{
    IntegrationConfig, IntegrationError, IntegrationServer, start_integration_server,
};
pub use intelligence::{
    IntelligenceConfidence, IntelligenceEvidence, IntelligenceState, RunIntelligence,
    derive_run_intelligence,
};
pub use launch_context::{
    BoundedItems, ContextV1, LaunchContextError, LaunchContextStore, MAX_CONTEXT_BYTES,
    REDACTED_CREDENTIAL, ScanBoundedItems, UNTRUSTED_DATA_NOTICE, build_launch_context,
    contains_potential_credential,
};
pub use persistence::{
    IntegrationDescriptor, PersistenceError, publish_runtime_json, read_integration_descriptor,
    remove_runtime_file, remove_runtime_json_if_equal, run_history_path, runtime_dir,
};
pub use privacy::{
    EventPrivacyError, EventPrivacyPolicy, default_event_privacy_policy,
    normalize_event_observation, retain_events,
};
pub use process::process_alive;
pub use registry::{
    AdmissionFence, DEFAULT_LEASE_DURATION, DEFAULT_MAX_RECORDS, DEFAULT_SNAPSHOT_LIMIT,
    DEFAULT_SWEEP_INTERVAL, Lease, LinkedAssociationChange, RealRegistryTicker, Registration,
    Registry, RegistryConfig, RegistryError, RegistryMutationOutcome, RegistryTicker,
};
pub use run::{Exit, LeaseState, ProcessState, RegistrationKind, Run, RunState};
pub use timestamp::Timestamp;

#[cfg(test)]
mod activity_test;
#[cfg(test)]
mod adapter_test;
#[cfg(test)]
mod association_test;
#[cfg(test)]
mod coordination_test;
#[cfg(test)]
mod correlation_test;
#[cfg(test)]
mod event_test;
#[cfg(test)]
mod handoff_test;
#[cfg(test)]
mod integration_test;
#[cfg(test)]
mod intelligence_test;
#[cfg(test)]
mod launch_context_test;
#[cfg(test)]
mod persistence_test;
#[cfg(test)]
mod privacy_test;
#[cfg(test)]
mod process_test;
#[cfg(test)]
mod registry_test;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod timestamp_test;
