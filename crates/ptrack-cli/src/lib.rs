//! Cobra-compatible p-track command parsing and process dispatch.

mod command;
mod compat_json;
mod completion;
mod dispatch;
mod error;
mod help;
mod output;
mod parse;
mod tree;

use std::ffi::OsString;
use std::io::{Read, Write};

pub use error::CliError;
use ptrack_app::{ApplicationPort, CapabilityCancellation};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunOutcome {
    ExitSuccess,
    LaunchTui,
    LaunchGui { path: String, plan_id: u64 },
}

pub struct Io<'a> {
    pub stdin: Box<dyn Read + Send>,
    pub stdout: &'a mut dyn Write,
    pub stderr: &'a mut dyn Write,
    pub cancellation: CapabilityCancellation,
}

/// Parses and executes one process invocation. Errors contain only the bare
/// compatibility message; the process owner prints it once to stderr.
///
/// # Errors
///
/// Returns [`CliError`] for an invalid command line, refused application use
/// case, failed output write, or compatibility-preserving command failure.
pub fn run<I, T>(
    args_os: I,
    application: &mut dyn ApplicationPort,
    io: Io<'_>,
) -> Result<RunOutcome, CliError>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    dispatch::run(args_os, application, io)
}

#[must_use]
pub const fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[must_use]
pub const fn no_project_hint() -> &'static str {
    concat!(
        "p-track  ·  persistent project memory\n",
        "──────────────────────────────────────\n",
        "\n",
        "No p-track project here yet.\n",
        "\n",
        "GET STARTED\n",
        "  ptrack init                 create one in this directory (or the git root)\n",
        "  ptrack init --goal \"...\"     create one and set the goal\n",
        "  ptrack --help               browse all commands\n",
        "\n",
        "Once a project exists, run `ptrack` to open the dashboard.\n",
    )
}

#[cfg(test)]
mod compat_json_test;
#[cfg(test)]
mod dispatch_test;
#[cfg(test)]
mod parse_test;
