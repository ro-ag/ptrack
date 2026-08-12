use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::Path;

use ptrack_store::{
    Collection, ImportCollection, ImportData, ImportRecord, ImportReport, OwnedRecordKey,
    RecordEnvelope, Store, StoreError, StoreKind,
};

use crate::{BundleError, BundleKind, ValidatedBundle, validate_path};

/// A bundle conversion, destination preflight, or destination-store failure.
#[derive(Debug)]
pub enum MigrationError {
    Bundle(BundleError),
    InvalidDestination(String),
    InvalidBundle(String),
    Store(StoreError),
}

impl fmt::Display for MigrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bundle(error) => error.fmt(formatter),
            Self::InvalidDestination(message) => {
                write!(formatter, "invalid migration destination: {message}")
            }
            Self::InvalidBundle(message) => {
                write!(formatter, "invalid migration bundle: {message}")
            }
            Self::Store(error) => write!(formatter, "cannot create imported database: {error}"),
        }
    }
}

impl Error for MigrationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Bundle(error) => Some(error),
            Self::Store(error) => Some(error),
            Self::InvalidDestination(_) | Self::InvalidBundle(_) => None,
        }
    }
}

impl From<BundleError> for MigrationError {
    fn from(value: BundleError) -> Self {
        Self::Bundle(value)
    }
}

impl From<StoreError> for MigrationError {
    fn from(value: StoreError) -> Self {
        Self::Store(value)
    }
}

/// Converts a fully validated legacy bundle into one complete destination image.
///
/// Historical project bundles are expanded to the current closed collection
/// set. Collections that did not exist in the source format are represented as
/// empty, with a zero sequence for every sequenced project collection.
///
/// # Errors
///
/// Returns [`MigrationError`] if a validated bundle cannot be represented by
/// the current destination schema.
pub fn bundle_into_import_data(bundle: ValidatedBundle) -> Result<ImportData, MigrationError> {
    let bundle_kind = bundle.kind();
    let source_format = u32::try_from(bundle.source_format()).map_err(|_| {
        MigrationError::InvalidBundle("source format does not fit the record envelope".to_owned())
    })?;
    let store_kind = store_kind(bundle_kind);
    let mut buckets = BTreeMap::new();

    for bucket in bundle.into_buckets() {
        let (name, sequence, records) = bucket.into_parts();
        let collection = Collection::from_legacy_name(name.as_bytes()).ok_or_else(|| {
            MigrationError::InvalidBundle(format!("unknown legacy collection {name:?}"))
        })?;
        if collection.store_kind() != store_kind {
            return Err(MigrationError::InvalidBundle(format!(
                "legacy collection {name:?} belongs to the wrong database family"
            )));
        }
        if buckets.insert(collection, (sequence, records)).is_some() {
            return Err(MigrationError::InvalidBundle(format!(
                "legacy collection {name:?} appears more than once"
            )));
        }
    }

    let collections = Collection::for_store(store_kind)
        .map(|collection| {
            let (sequence, records) = buckets
                .remove(&collection)
                .unwrap_or_else(|| (0, Vec::new()));
            let records = records
                .into_iter()
                .map(|record| {
                    let (key, value) = record.into_parts();
                    Ok(ImportRecord {
                        key: import_key(collection, key)?,
                        envelope: RecordEnvelope::new(
                            collection.legacy_codec(),
                            source_format,
                            value,
                        ),
                    })
                })
                .collect::<Result<Vec<_>, MigrationError>>()?;
            Ok(ImportCollection {
                collection,
                records,
                sequence: collection.is_sequenced().then_some(sequence),
            })
        })
        .collect::<Result<Vec<_>, MigrationError>>()?;

    if !buckets.is_empty() {
        return Err(MigrationError::InvalidBundle(
            "bundle contains collections outside the destination schema".to_owned(),
        ));
    }

    Ok(ImportData {
        kind: store_kind,
        collections,
    })
}

/// Imports one already-validated bundle into one explicitly named absent file.
///
/// # Errors
///
/// Returns [`MigrationError`] when the destination is unsafe, conversion
/// fails, or the destination store cannot complete and verify the import.
pub fn import_validated_bundle(
    bundle: ValidatedBundle,
    destination: &Path,
) -> Result<(Store, ImportReport), MigrationError> {
    validate_destination(destination, None)?;
    let data = bundle_into_import_data(bundle)?;
    Store::import_new(destination, data).map_err(MigrationError::from)
}

/// Validates an explicit bundle completely, then imports it to an explicit path.
///
/// Bundle validation always completes before any destination is created.
///
/// # Errors
///
/// Returns [`MigrationError`] when either path or the bundle is invalid, or the
/// destination store cannot complete and verify the import.
pub fn import_path(
    bundle_path: &Path,
    destination: &Path,
) -> Result<(Store, ImportReport), MigrationError> {
    let bundle = validate_path(bundle_path)?;
    validate_destination(destination, Some(bundle_path))?;
    let data = bundle_into_import_data(bundle)?;
    Store::import_new(destination, data).map_err(MigrationError::from)
}

const fn store_kind(kind: BundleKind) -> StoreKind {
    match kind {
        BundleKind::Project => StoreKind::Project,
        BundleKind::Global => StoreKind::Global,
    }
}

fn import_key(collection: Collection, key: Vec<u8>) -> Result<OwnedRecordKey, MigrationError> {
    match collection {
        Collection::ProjectMeta => {
            if key == b"meta" {
                Ok(OwnedRecordKey::Singleton)
            } else {
                Err(MigrationError::InvalidBundle(
                    "project meta key is not the singleton meta key".to_owned(),
                ))
            }
        }
        Collection::Plans
        | Collection::Tasks
        | Collection::Notes
        | Collection::Milestones
        | Collection::Issues
        | Collection::Commits
        | Collection::Capabilities
        | Collection::CapabilityAudits => {
            let bytes: [u8; 8] = key.try_into().map_err(|key: Vec<u8>| {
                MigrationError::InvalidBundle(format!(
                    "collection {} contains a {}-byte numeric key",
                    collection.name(),
                    key.len()
                ))
            })?;
            let id = u64::from_be_bytes(bytes);
            if id == 0 {
                return Err(MigrationError::InvalidBundle(format!(
                    "collection {} contains numeric ID zero",
                    collection.name()
                )));
            }
            Ok(OwnedRecordKey::Id(id))
        }
        Collection::MemoryWritebacks
        | Collection::GlobalConfig
        | Collection::GlobalProjects
        | Collection::GlobalBackups => Ok(OwnedRecordKey::Bytes(key)),
    }
}

fn validate_destination(
    destination: &Path,
    bundle_path: Option<&Path>,
) -> Result<(), MigrationError> {
    if !destination.is_absolute() {
        return invalid_destination("path must be absolute");
    }
    let extension = destination
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .ok_or_else(|| MigrationError::InvalidDestination("path must end in .redb".to_owned()))?;
    if !extension.eq_ignore_ascii_case("redb") {
        return invalid_destination("path must end in .redb");
    }
    if destination
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|name| {
            name.eq_ignore_ascii_case("ptrack.db") || name.eq_ignore_ascii_case("global.db")
        })
    {
        return invalid_destination("legacy bbolt filenames are forbidden");
    }
    if bundle_path.is_some_and(|bundle| paths_resolve_equal(bundle, destination)) {
        return invalid_destination("bundle and destination paths must differ");
    }
    match fs::symlink_metadata(destination) {
        Ok(_) => return invalid_destination("path already exists"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return invalid_destination_owned(format!("cannot inspect path: {error}"));
        }
    }

    let parent = destination.parent().ok_or_else(|| {
        MigrationError::InvalidDestination("path has no parent directory".to_owned())
    })?;
    let metadata = fs::symlink_metadata(parent).map_err(|error| {
        MigrationError::InvalidDestination(format!("parent directory is unavailable: {error}"))
    })?;
    if metadata.file_type().is_symlink() {
        return invalid_destination("parent directory must not be a symbolic link");
    }
    if !metadata.is_dir() {
        return invalid_destination("parent path must be an existing directory");
    }
    Ok(())
}

fn paths_resolve_equal(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    let Some(right_parent) = right.parent() else {
        return false;
    };
    let Some(right_name) = right.file_name() else {
        return false;
    };
    let Ok(left) = fs::canonicalize(left) else {
        return false;
    };
    let Ok(parent) = fs::canonicalize(right_parent) else {
        return false;
    };
    left == parent.join(right_name)
}

fn invalid_destination<T>(message: &str) -> Result<T, MigrationError> {
    invalid_destination_owned(message.to_owned())
}

fn invalid_destination_owned<T>(message: String) -> Result<T, MigrationError> {
    Err(MigrationError::InvalidDestination(message))
}
