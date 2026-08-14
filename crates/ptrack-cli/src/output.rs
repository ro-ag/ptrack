use std::io::Write;

use serde::Serialize;

use crate::error::CliError;

pub fn text(output: &mut dyn Write, value: &str) -> Result<(), CliError> {
    output.write_all(value.as_bytes())?;
    Ok(())
}

pub fn line(output: &mut dyn Write, value: impl std::fmt::Display) -> Result<(), CliError> {
    writeln!(output, "{value}")?;
    Ok(())
}

pub fn json<T: Serialize>(output: &mut dyn Write, value: &T) -> Result<(), CliError> {
    let mut encoded = serde_json::to_string_pretty(value)
        .map_err(|error| CliError::message(error.to_string()))?;
    // Go encoding/json escapes HTML delimiters plus the JavaScript line and
    // paragraph separators even when they are otherwise valid UTF-8.
    encoded = encoded
        .replace('&', "\\u0026")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029");
    writeln!(output, "{encoded}")?;
    Ok(())
}
