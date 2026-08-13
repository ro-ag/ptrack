use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CliError(String);

impl CliError {
    #[must_use]
    pub fn message(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for CliError {}

impl From<ptrack_app::AppError> for CliError {
    fn from(error: ptrack_app::AppError) -> Self {
        Self(error.to_string())
    }
}

impl From<std::io::Error> for CliError {
    fn from(error: std::io::Error) -> Self {
        Self(error.to_string())
    }
}

impl From<ptrack_core::ReportError> for CliError {
    fn from(_error: ptrack_core::ReportError) -> Self {
        // Go's report commands surface store.ErrNotFound without adding the
        // requested entity kind or id.
        Self("not found".to_owned())
    }
}
