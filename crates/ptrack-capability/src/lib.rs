#![deny(unsafe_code)]

//! Capability facade.
//!
//! Pure policy and opaque authorization evidence live in the inward
//! `ptrack-capability-policy` crate. Persistence depends only on that inward
//! crate; this outward facade may compose policy and storage without a cycle.

mod audit;
mod broker;
mod diagnostics;
mod git;
mod http;
mod mcp;
#[cfg(windows)]
mod private_windows;
mod process;
mod server;
mod ssh;

pub use audit::{AuditError, AuditRecorder};
pub use broker::{
    Broker, BrokerConfig, BrokerError, SessionIdentity, TOOL_GIT, TOOL_HTTP_REQUEST, TOOL_SSH,
    ToolCall, ToolDefinition, tool_definitions,
};
pub use diagnostics::{
    ConnectionDiagnostic, ConnectionTester, VpnState, VpnUnavailableError, detect_vpn_state,
};
pub use git::{GitError, GitExecutor, GitRequest, GitResult, classify_git_exit};
pub use http::{
    ConnectionClass, HttpDiagnostics, HttpError, HttpExecutor, HttpRequest, HttpResponse,
};
pub use mcp::{MCP_PROTOCOL_VERSION, McpServeOutcome, serve_mcp};
pub use server::{
    BrokerClient, BrokerDescriptor, BrokerServer, BrokerServerConfig, ServerError,
    SessionEnvironment, client_for_project, read_broker_descriptor, validate_session_environment,
};
pub use ssh::{
    SshError, SshExecutor, SshRequest, SshResult, classify_ssh_error, classify_ssh_exit,
};
pub use tokio_util::sync::CancellationToken as McpCancellation;

#[cfg(test)]
mod audit_test;
#[cfg(test)]
mod broker_test;
#[cfg(test)]
mod contract_coverage_test;
#[cfg(test)]
mod diagnostics_test;
#[cfg(test)]
mod git_test;
#[cfg(test)]
mod http_test;
#[cfg(test)]
mod mcp_test;
#[cfg(all(test, windows))]
mod private_windows_test;
#[cfg(test)]
mod process_test;
#[cfg(test)]
mod server_test;
#[cfg(test)]
mod ssh_test;
#[cfg(test)]
mod test_support;
pub use ptrack_capability_policy::*;
