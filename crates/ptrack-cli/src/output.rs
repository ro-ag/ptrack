use std::borrow::Cow;
use std::io::Write;

use ptrack_core::is_forbidden_control;
use serde::Serialize;

use crate::error::CliError;

/// Neutralizes terminal escapes and other invisible controls in one output
/// line before it reaches a terminal.
///
/// Line breaks and tabs are structural whitespace real output already
/// contains — a goal, summary, or note body spans lines by design — so they
/// pass through. Every other character `is_forbidden_control` refuses (C0/C1
/// including CR and ESC, the line and paragraph separators, the bidi
/// controls, and the zero-width and tag characters) becomes a space. Stored
/// text is untrusted: a record written before validation existed must not be
/// able to repaint a terminal, forge a line, or hide part of what it prints.
fn sanitize(value: &str) -> Cow<'_, str> {
    let forbidden =
        |character: char| is_forbidden_control(character) && character != '\n' && character != '\t';
    if value.chars().any(forbidden) {
        Cow::Owned(
            value
                .chars()
                .map(|character| if forbidden(character) { ' ' } else { character })
                .collect(),
        )
    } else {
        Cow::Borrowed(value)
    }
}

/// Renders a path the way a person writes it. Windows canonicalization
/// yields verbatim `\\?\C:\…` and `\\?\UNC\…` forms; they stay exact in
/// storage, but output shows `C:\…` and `\\server\…`. Other verbatim forms
/// (volume GUIDs, devices) have no plain spelling and print unchanged.
pub fn path(value: &std::path::Path) -> String {
    let rendered = value.display().to_string();
    if let Some(rest) = rendered.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{rest}");
    }
    match rendered.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => rest.to_owned(),
        _ => rendered,
    }
}

pub fn text(output: &mut dyn Write, value: &str) -> Result<(), CliError> {
    output.write_all(sanitize(value).as_bytes())?;
    Ok(())
}

pub fn line(output: &mut dyn Write, value: impl std::fmt::Display) -> Result<(), CliError> {
    let value = value.to_string();
    writeln!(output, "{}", sanitize(&value))?;
    Ok(())
}

pub fn json<T: Serialize>(output: &mut dyn Write, value: &T) -> Result<(), CliError> {
    let mut encoded = serde_json::to_string_pretty(value)
        .map_err(|error| CliError::message(error.to_string()))?;
    // JSON output escapes HTML delimiters plus the JavaScript line and
    // paragraph separators even when they are otherwise valid UTF-8, so
    // existing consumers see identical bytes.
    encoded = encoded
        .replace('&', "\\u0026")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029");
    writeln!(output, "{encoded}")?;
    Ok(())
}
