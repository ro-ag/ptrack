#![deny(unsafe_code)]

//! Bounded terminal sessions, profiles, streams, and shell integration.

mod manager;
mod profile;
mod profile_config;
mod protocol;
mod pty;
mod session;
mod shell_integration;
mod stream;

pub use manager::*;
pub use profile::*;
pub use profile_config::*;
pub use protocol::*;
pub use pty::*;
pub use session::*;
pub use shell_integration::*;
pub use stream::*;

#[cfg(test)]
mod manager_test;
#[cfg(test)]
mod profile_config_test;
#[cfg(test)]
mod profile_test;
#[cfg(test)]
mod protocol_test;
#[cfg(test)]
mod pty_test;
#[cfg(test)]
mod session_test;
#[cfg(test)]
mod shell_integration_test;
#[cfg(test)]
mod stream_test;
