use std::error::Error;
use std::fmt;
use std::io;

/// A staging validation or disabled-candidate-creation error.
#[derive(Debug)]
pub enum ImportError {
    /// A filesystem read failed.
    Io(io::Error),
    /// JSON syntax or the closed staging schema is invalid.
    InvalidStage(String),
    /// The create-only redb store rejected or could not verify a candidate.
    Store(ptrack_store::StoreError),
    /// The explicit destructive-boundary acknowledgement was omitted.
    AcceptanceRequired,
}

impl fmt::Display for ImportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "cannot read database stage: {error}"),
            Self::InvalidStage(detail) => write!(formatter, "invalid database stage: {detail}"),
            Self::Store(error) => write!(
                formatter,
                "cannot create verified database candidate: {error}"
            ),
            Self::AcceptanceRequired => formatter.write_str("--accept-all is required"),
        }
    }
}

impl Error for ImportError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Store(error) => Some(error),
            Self::InvalidStage(_) | Self::AcceptanceRequired => None,
        }
    }
}

impl From<io::Error> for ImportError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<ptrack_store::StoreError> for ImportError {
    fn from(value: ptrack_store::StoreError) -> Self {
        Self::Store(value)
    }
}

/// Result alias for database staging operations.
pub type ImportResult<T> = Result<T, ImportError>;

pub(crate) fn invalid<T>(detail: impl Into<String>) -> ImportResult<T> {
    Err(ImportError::InvalidStage(detail.into()))
}
