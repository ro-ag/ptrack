//! Positional argument access for the desktop command parser.
//!
//! Every accessor reproduces the exact error text the bridge has always
//! returned for a missing or mistyped argument, so moving the checks into one
//! parser changed no message a caller can observe.

use std::path::PathBuf;

use ptrack_terminal::TerminalAssociationPointer;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::{RECENT_PROJECT_PATH_LIMIT, RECENT_PROJECT_TOKEN_BYTES};
use crate::terminal_windows::TerminalWindowTab;
use crate::{AppError, AppResult};

/// One request's positional arguments, read by index.
#[derive(Clone, Copy)]
pub(super) struct Args<'a> {
    method: &'a str,
    values: &'a [Value],
}

impl<'a> Args<'a> {
    pub(super) const fn new(method: &'a str, values: &'a [Value]) -> Self {
        Self { method, values }
    }

    pub(super) const fn len(self) -> usize {
        self.values.len()
    }

    /// Requires exactly `expected` arguments.
    pub(super) fn exact(self, expected: usize) -> AppResult<()> {
        if self.values.len() == expected {
            Ok(())
        } else {
            Err(AppError::Message(format!(
                "{} requires exactly {expected} arguments",
                self.method
            )))
        }
    }

    /// The raw JSON argument, for payloads the handler interprets itself.
    pub(super) fn value(self, index: usize) -> AppResult<&'a Value> {
        self.values.get(index).ok_or_else(|| missing_arg(index))
    }

    /// Whether the argument is present and JSON `null`.
    pub(super) fn is_null(self, index: usize) -> bool {
        self.values.get(index).is_some_and(Value::is_null)
    }

    pub(super) fn string(self, index: usize) -> AppResult<&'a str> {
        self.values
            .get(index)
            .and_then(Value::as_str)
            .ok_or_else(|| missing_arg(index))
    }

    pub(super) fn path(self, index: usize) -> AppResult<PathBuf> {
        Ok(PathBuf::from(self.string(index)?))
    }

    pub(super) fn strings(self, index: usize) -> AppResult<Vec<String>> {
        let values = self
            .values
            .get(index)
            .and_then(Value::as_array)
            .ok_or_else(|| missing_arg(index))?;
        values
            .iter()
            .enumerate()
            .map(|(offset, value)| {
                value.as_str().map(ToOwned::to_owned).ok_or_else(|| {
                    AppError::Message(format!(
                        "desktop command argument {index}[{offset}] is invalid"
                    ))
                })
            })
            .collect()
    }

    pub(super) fn u64(self, index: usize) -> AppResult<u64> {
        self.values
            .get(index)
            .and_then(Value::as_u64)
            .ok_or_else(|| missing_arg(index))
    }

    pub(super) fn i64(self, index: usize) -> AppResult<i64> {
        self.values
            .get(index)
            .and_then(Value::as_i64)
            .ok_or_else(|| missing_arg(index))
    }

    pub(super) fn u16(self, index: usize) -> AppResult<u16> {
        u16::try_from(self.u64(index)?).map_err(|_| missing_arg(index))
    }

    pub(super) fn bool(self, index: usize) -> AppResult<bool> {
        self.values
            .get(index)
            .and_then(Value::as_bool)
            .ok_or_else(|| missing_arg(index))
    }

    /// A boolean that only some forms of a command require; the handler
    /// reports [`missing_arg`] when its form needs one that is absent.
    pub(super) fn optional_bool(self, index: usize) -> Option<bool> {
        self.values.get(index).and_then(Value::as_bool)
    }

    /// A structured argument deserialized into `T`.
    pub(super) fn typed<T: DeserializeOwned>(self, index: usize) -> AppResult<T> {
        serde_json::from_value(self.value(index)?.clone()).map_err(|_| missing_arg(index))
    }

    /// A terminal association pointer, reporting the deserializer's own text.
    pub(super) fn pointer(self, index: usize) -> AppResult<TerminalAssociationPointer> {
        serde_json::from_value(self.value(index)?.clone())
            .map_err(|error| AppError::Message(error.to_string()))
    }

    /// A terminal-window tab from two adjacent arguments: the session list and
    /// the shape, which must be a JSON object — the frontend re-hydrates a
    /// split tree from it, and anything else could only ever render nothing.
    pub(super) fn tab(self, index: usize) -> AppResult<TerminalWindowTab> {
        let sessions = self.strings(index)?;
        let shape = self
            .values
            .get(index + 1)
            .filter(|value| value.is_object())
            .cloned()
            .ok_or_else(|| {
                AppError::Message(format!("{} requires an object tab shape", self.method))
            })?;
        Ok(TerminalWindowTab { sessions, shape })
    }

    /// An opaque recent-project entry identifier or base token.
    pub(super) fn recent_identifier(self, index: usize) -> AppResult<&'a str> {
        let value = self.string(index)?;
        if value.len() == RECENT_PROJECT_TOKEN_BYTES
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            Ok(value)
        } else {
            Err(missing_arg(index))
        }
    }

    /// A recent-project token that may be empty.
    pub(super) fn recent_optional_token(self, index: usize) -> AppResult<&'a str> {
        let value = self.string(index)?;
        if value.is_empty() {
            Ok(value)
        } else {
            self.recent_identifier(index)
        }
    }

    /// A bounded, non-empty recent-project candidate path.
    pub(super) fn recent_path(self, index: usize) -> AppResult<PathBuf> {
        let value = self.string(index)?;
        if value.is_empty() || value.len() > RECENT_PROJECT_PATH_LIMIT {
            return Err(missing_arg(index));
        }
        Ok(PathBuf::from(value))
    }
}

/// The error every missing or mistyped positional argument reports.
pub(super) fn missing_arg(index: usize) -> AppError {
    AppError::Message(format!("desktop command argument {index} is invalid"))
}
