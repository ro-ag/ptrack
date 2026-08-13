//! UI-neutral p-track application services.
//!
//! The service owns explicit roots, activation bindings, and writer identity.
//! Every operation opens its required store, verifies the binding, performs
//! one use case, and drops the handle before returning. No caller can retain a
//! redb handle or accidentally hold the writer lock while idle.

mod agent_runtime;
mod service;

pub use agent_runtime::{
    AgentAdmissionFence, AgentIntegration, AgentIntegrationFactory, AgentInvalidationV2,
    AgentMutationOutcome, AgentNotificationsV2, AgentResourceStateV2, AgentRuntime,
    AgentRuntimeConfig, AgentRuntimeService, AgentWorkflowTargetsV2, LaunchedEventAuthority,
    LinkedAgentAssociationChange, LinkedAgentRuntimeHooks, ProductionAgentIntegrationFactory,
    ProjectCoordinationStore, PtrackCoordinationGit,
};

pub use service::{
    AppError, AppResult, ApplicationPort, CapabilityMcpOutcome, GuideAction, HookAction,
    HookResult, InitRequest, InitResult, LocalApplication, Mutation, MutationResult, ProcessOutput,
    ProjectEndpoint, UnavailableApplication, WorkspaceBindings,
};

#[cfg(test)]
mod agent_runtime_test;
#[cfg(test)]
mod service_test;
