use std::error::Error;
use std::fmt;
use std::io;
use std::path::PathBuf;

use crate::schema::StoreKind;

/// The layer prefix [`StoreError::InvalidHold`] renders before its detail.
///
/// The detail is already a whole sentence ("task #3 is done and cannot be put
/// on hold"), so presentation layers strip this prefix instead of maintaining a
/// parallel message table. Exported so no caller has to hardcode a copy that
/// can drift away from the `Display` below.
pub const INVALID_HOLD_PREFIX: &str = "invalid hold mutation: ";

/// The layer prefix [`StoreError::InvalidClaim`] renders before its detail.
///
/// The detail is already a whole sentence ("plan #3 is claimed by ..."), so
/// presentation layers strip this prefix instead of maintaining a parallel
/// message table. Exported so no caller has to hardcode a copy that can drift
/// away from the `Display` below.
pub const INVALID_CLAIM_PREFIX: &str = "invalid claim mutation: ";

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
    /// The destination's parent is missing, linked, or not a directory.
    DestinationParentInvalid { path: PathBuf },
    /// The destination's parent directory changed during create-only work.
    DestinationParentChanged { path: PathBuf },
    /// This platform cannot prove destination-parent identity with safe std APIs.
    DestinationParentIdentityUnavailable { path: PathBuf },
    /// Import reached `ready`, but its destination namespace changed afterward.
    ImportCommittedPathChanged { path: PathBuf },
    /// Import reached `ready`, but a later durability or validation check failed.
    ImportCommittedVerificationFailed { path: PathBuf, detail: String },
    /// The engine could not prove whether the final ready transaction committed.
    ImportCommitOutcomeUnknown { path: PathBuf, detail: String },
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
    /// A store is not bound to the exact active runtime generation.
    ActivationBinding(String),
    /// A typed record was not found.
    NotFound,
    /// A compare-and-set task fence observed different state.
    TaskStatusChanged(String),
    /// A capability draft or lifecycle mutation observed a stale revision.
    CapabilityRevisionChanged { expected: u64, actual: u64 },
    /// Approval was attempted against a digest other than the stored preview.
    CapabilityScopeChanged,
    /// Expiry was requested for a capability which is not currently enabled.
    CapabilityNotEnabled,
    /// A bounded read used an unsafe resource limit.
    InvalidBoundedLimit,
    /// A bounded aggregate would traverse more rows than its hard ceiling.
    BoundedScanLimit {
        collection: &'static str,
        maximum: usize,
    },
    /// A caller-owned presentation deadline expired during a bounded scan.
    DeadlineExceeded,
    /// A memory write-back request is malformed or its target is stale.
    InvalidMemoryWriteback(String),
    /// A memory request identifier was reused for different content.
    MemoryWritebackReplay,
    /// A generation-scoped request belongs to another workspace generation.
    StaleWorkspaceGeneration { expected: u64, active: u64 },
    /// A first-run plan or task request is invalid or conflicts with durable state.
    InvalidFirstRun(String),
    /// A hold request targets a plan or task that has already reached a
    /// terminal state. Resuming is always allowed; only holding is refused.
    InvalidHold(String),
    /// A claim-gated mutation was attempted against a plan claimed by someone
    /// else, or a claim operation was invalid for the caller's identity.
    InvalidClaim(String),
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
            Self::DestinationParentInvalid { path } => write!(
                formatter,
                "database destination parent must be a real directory: {}",
                path.display()
            ),
            Self::DestinationParentChanged { path } => write!(
                formatter,
                "database destination parent changed during creation: {}",
                path.display()
            ),
            Self::DestinationParentIdentityUnavailable { path } => write!(
                formatter,
                "database destination parent identity cannot be verified on this platform: {}",
                path.display()
            ),
            Self::ImportCommittedPathChanged { path } => write!(
                formatter,
                "database import committed, but its destination path changed afterward: {}",
                path.display()
            ),
            Self::ImportCommittedVerificationFailed { path, detail } => write!(
                formatter,
                "database import committed, but final verification failed for {}: {detail}",
                path.display()
            ),
            Self::ImportCommitOutcomeUnknown { path, detail } => write!(
                formatter,
                "database import commit outcome is unknown for {}: {detail}",
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
            Self::ActivationBinding(detail) => {
                write!(formatter, "invalid activation binding: {detail}")
            }
            Self::NotFound => formatter.write_str("not found"),
            Self::TaskStatusChanged(detail) => {
                write!(formatter, "task status changed: {detail}")
            }
            Self::CapabilityRevisionChanged { expected, actual } => write!(
                formatter,
                "capability revision changed: expected {expected}, found {actual}"
            ),
            Self::CapabilityScopeChanged => {
                formatter.write_str("effective scope changed; preview again before enabling")
            }
            Self::CapabilityNotEnabled => {
                formatter.write_str("only an enabled capability can be expired")
            }
            Self::InvalidBoundedLimit => {
                formatter.write_str("bounded read limit must be between 1 and 1000")
            }
            Self::BoundedScanLimit {
                collection,
                maximum,
            } => write!(
                formatter,
                "bounded {collection} scan exceeds its limit of {maximum} records"
            ),
            Self::DeadlineExceeded => formatter.write_str("context deadline exceeded"),
            Self::InvalidMemoryWriteback(detail) => {
                write!(formatter, "invalid memory write-back: {detail}")
            }
            Self::MemoryWritebackReplay => {
                formatter.write_str("memory write-back request ID was already used")
            }
            Self::StaleWorkspaceGeneration { expected, active } => write!(
                formatter,
                "stale workspace generation: expected {expected}, active {active}"
            ),
            Self::InvalidFirstRun(detail) => {
                write!(formatter, "invalid first-run mutation: {detail}")
            }
            Self::InvalidHold(detail) => {
                write!(formatter, "{INVALID_HOLD_PREFIX}{detail}")
            }
            Self::InvalidClaim(detail) => {
                write!(formatter, "{INVALID_CLAIM_PREFIX}{detail}")
            }
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
