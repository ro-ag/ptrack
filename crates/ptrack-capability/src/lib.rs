#![forbid(unsafe_code)]

//! Capability facade.
//!
//! Pure policy and opaque authorization evidence live in the inward
//! `ptrack-capability-policy` crate. Persistence depends only on that inward
//! crate; this outward facade may compose policy and storage without a cycle.

mod audit;
mod git;
mod http;
mod process;

pub use audit::{AuditError, AuditRecorder};
pub use git::{GitError, GitExecutor, GitRequest, GitResult, classify_git_exit};
pub use http::{
    ConnectionClass, HttpDiagnostics, HttpError, HttpExecutor, HttpRequest, HttpResponse,
};
pub use ptrack_capability_policy::*;
