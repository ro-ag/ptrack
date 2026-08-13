use std::io::Write;

use clap_complete::{Shell, generate};

use crate::error::CliError;

pub fn write(shell: &str, no_descriptions: bool, output: &mut dyn Write) -> Result<(), CliError> {
    let shell = match shell {
        "bash" => Shell::Bash,
        "zsh" => Shell::Zsh,
        "fish" => Shell::Fish,
        "powershell" => Shell::PowerShell,
        other => return Err(CliError::message(format!("unsupported shell {other:?}"))),
    };
    let mut command = crate::tree::root();
    if no_descriptions {
        command = command.disable_help_flag(true);
    }
    generate(shell, &mut command, "ptrack", output);
    Ok(())
}
