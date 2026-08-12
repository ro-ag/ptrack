use std::error::Error;
use std::fmt;
use std::io;
use std::path::PathBuf;

use crate::schema::StoreKind;

/// An error encountered while decoding a persisted record envelope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EnvelopeError {
    /// The input ended before the fixed-width envelope header was complete.
    HeaderTooShort { actual: usize, minimum: usize },
    /// The input does not begin with the ptrack record magic bytes.
    InvalidMagic { actual: [u8; 4] },
    /// The record uses an envelope layout this build does not understand.
    UnsupportedEnvelopeVersion { actual: u16, supported: u16 },
    /// The declared payload size cannot be represented by this process.
    PayloadLengthOverflow { declared: u64 },
    /// The input ended before the declared payload was complete.
    PayloadTooShort { declared: u64, actual: usize },
    /// Bytes remain after the declared payload.
    TrailingBytes { declared: u64, trailing: usize },
}

impl fmt::Display for EnvelopeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HeaderTooShort { actual, minimum } => write!(
                formatter,
                "record envelope header is too short: got {actual} bytes, need at least {minimum}"
            ),
            Self::InvalidMagic { actual } => write!(
                formatter,
                "invalid record envelope magic: got {:02x?}",
                actual
            ),
            Self::UnsupportedEnvelopeVersion { actual, supported } => write!(
                formatter,
                "unsupported record envelope version {actual}; this build supports {supported}"
            ),
            Self::PayloadLengthOverflow { declared } => write!(
                formatter,
                "declared record payload length {declared} cannot be represented on this platform"
            ),
            Self::PayloadTooShort { declared, actual } => write!(
                formatter,
                "record payload is too short: header declares {declared} bytes, got {actual}"
            ),
            Self::TrailingBytes { declared, trailing } => write!(
                formatter,
                "record has {trailing} trailing bytes after its declared {declared}-byte payload"
            ),
        }
    }
}

impl Error for EnvelopeError {}

/// An error from the versioned ptrack destination store.
#[derive(Debug)]
pub enum StoreError {
    /// A create-only operation was pointed at an existing destination.
    DestinationExists { path: PathBuf },
    /// The requested database path is a symbolic link.
    SymbolicLink { path: PathBuf },
    /// The requested database path is not a regular file.
    NotRegularFile { path: PathBuf },
    /// The database pathname changed while it was being opened or cleaned up.
    PathChanged { path: PathBuf },
    /// A Rust destination was pointed at a reserved legacy bbolt filename.
    LegacyPathForbidden { path: PathBuf },
    /// A database file exposes project data to group or other users.
    InsecurePermissions { path: PathBuf, mode: u32 },
    /// Another process owns the database's exclusive writer lock.
    Busy,
    /// Database metadata is missing, malformed, or internally inconsistent.
    InvalidManifest(String),
    /// The database belongs to the other ptrack store family.
    WrongStoreKind {
        expected: StoreKind,
        actual: StoreKind,
    },
    /// The application schema cannot be opened by this build.
    UnsupportedSchemaVersion { actual: u32, current: u32 },
    /// A collection was used with the wrong project/global database.
    CollectionStoreMismatch {
        collection: &'static str,
        expected: StoreKind,
        actual: StoreKind,
    },
    /// A collection was addressed with the wrong key representation.
    KeyKindMismatch {
        collection: &'static str,
        expected: &'static str,
        actual: &'static str,
    },
    /// A sequence operation was requested for an unsequenced collection.
    SequenceNotSupported { collection: &'static str },
    /// A sequence high-water mark may never move backwards.
    SequenceWouldDecrease {
        collection: &'static str,
        current: u64,
        requested: u64,
    },
    /// A collection has exhausted its 64-bit sequence space.
    SequenceOverflow { collection: &'static str },
    /// A staged legacy import was incomplete or internally inconsistent.
    InvalidImport(String),
    /// A staged legacy import exceeded a fixed resource bound.
    ImportLimitExceeded {
        limit: &'static str,
        maximum: u64,
        actual: u64,
    },
    /// A failed mutating operation prevents this transaction from committing.
    TransactionPoisoned,
    /// A stored record envelope was invalid.
    Envelope(EnvelopeError),
    /// A filesystem operation failed.
    Io(io::Error),
    /// The redb storage engine rejected an operation.
    Engine(redb::Error),
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DestinationExists { path } => {
                write!(
                    formatter,
                    "database destination already exists: {}",
                    path.display()
                )
            }
            Self::SymbolicLink { path } => {
                write!(
                    formatter,
                    "database path is a symbolic link: {}",
                    path.display()
                )
            }
            Self::NotRegularFile { path } => {
                write!(
                    formatter,
                    "database path is not a regular file: {}",
                    path.display()
                )
            }
            Self::PathChanged { path } => write!(
                formatter,
                "database path changed while it was being opened: {}",
                path.display()
            ),
            Self::LegacyPathForbidden { path } => write!(
                formatter,
                "Rust database path may not use a legacy bbolt filename: {}",
                path.display()
            ),
            Self::InsecurePermissions { path, mode } => write!(
                formatter,
                "database permissions must not grant group or other access: {} has mode {mode:o}",
                path.display()
            ),
            Self::Busy => write!(formatter, "database is busy"),
            Self::InvalidManifest(detail) => {
                write!(formatter, "invalid database manifest: {detail}")
            }
            Self::WrongStoreKind { expected, actual } => write!(
                formatter,
                "wrong database kind: expected {expected}, found {actual}"
            ),
            Self::UnsupportedSchemaVersion { actual, current } => write!(
                formatter,
                "unsupported ptrack database schema {actual}; this build supports {current}"
            ),
            Self::CollectionStoreMismatch {
                collection,
                expected,
                actual,
            } => write!(
                formatter,
                "collection {collection} belongs to a {expected} database, not {actual}"
            ),
            Self::KeyKindMismatch {
                collection,
                expected,
                actual,
            } => write!(
                formatter,
                "collection {collection} requires a {expected} key, not {actual}"
            ),
            Self::SequenceNotSupported { collection } => {
                write!(
                    formatter,
                    "collection {collection} does not have a sequence"
                )
            }
            Self::SequenceWouldDecrease {
                collection,
                current,
                requested,
            } => write!(
                formatter,
                "collection {collection} sequence cannot decrease from {current} to {requested}"
            ),
            Self::SequenceOverflow { collection } => {
                write!(formatter, "collection {collection} sequence has overflowed")
            }
            Self::InvalidImport(detail) => write!(formatter, "invalid database import: {detail}"),
            Self::ImportLimitExceeded {
                limit,
                maximum,
                actual,
            } => write!(
                formatter,
                "database import {limit} exceeds its limit: got {actual}, maximum {maximum}"
            ),
            Self::TransactionPoisoned => write!(
                formatter,
                "transaction contains a failed mutation and cannot be committed"
            ),
            Self::Envelope(error) => error.fmt(formatter),
            Self::Io(error) => error.fmt(formatter),
            Self::Engine(error) => error.fmt(formatter),
        }
    }
}

impl Error for StoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Envelope(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::Engine(error) => Some(error),
            _ => None,
        }
    }
}

impl From<EnvelopeError> for StoreError {
    fn from(error: EnvelopeError) -> Self {
        Self::Envelope(error)
    }
}

impl From<io::Error> for StoreError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<redb::Error> for StoreError {
    fn from(error: redb::Error) -> Self {
        Self::Engine(error)
    }
}

impl From<redb::DatabaseError> for StoreError {
    fn from(error: redb::DatabaseError) -> Self {
        match error {
            redb::DatabaseError::DatabaseAlreadyOpen => Self::Busy,
            other => Self::Engine(other.into()),
        }
    }
}

macro_rules! engine_error_conversion {
    ($($source:ty),+ $(,)?) => {
        $(
            impl From<$source> for StoreError {
                fn from(error: $source) -> Self {
                    Self::Engine(error.into())
                }
            }
        )+
    };
}

engine_error_conversion!(
    redb::CommitError,
    redb::SetDurabilityError,
    redb::StorageError,
    redb::TableError,
    redb::TransactionError,
);

/// Result type for ptrack destination-store operations.
pub type StoreResult<T> = Result<T, StoreError>;
