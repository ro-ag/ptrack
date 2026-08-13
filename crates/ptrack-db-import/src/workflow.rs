use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path};

use ptrack_store::{JsonStageProvenance, Store};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{ImportError, ImportResult, invalid};
use crate::manifest::{DatabaseKind, clean_absolute};
use crate::sha256::hex;
use crate::stage::{StageReport, load_stage};

const RECEIPT_NAME: &str = "receipt.json";
const INCOMPLETE_NAME: &str = "incomplete.json";

/// Durable summary written only after every candidate was created and reopened.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportReceipt {
    pub report: StageReport,
    pub candidate_count: u64,
}

#[derive(Serialize)]
struct Receipt<'a> {
    format: &'static str,
    version: &'static str,
    manifest_sha256: String,
    database_count: String,
    quarantine_count: String,
    candidates: &'a [ReceiptCandidate],
}

#[derive(Deserialize, Serialize)]
struct ReceiptCandidate {
    id: String,
    kind: &'static str,
    path: String,
    source_format: String,
    database_json_sha256: String,
    quarantine_count: String,
    file_sha256: String,
}

/// Validates an immutable JSON stage, then creates verified inert redb candidates.
///
/// Validation completes before the destination path is inspected or created. The
/// destination must be an absolute clean path that does not exist, and `accept_all`
/// must be true. A durable receipt is published only after every candidate reopens
/// with exact JSON-stage provenance.
///
/// # Errors
///
/// Returns an error for an invalid stage, omitted acceptance, unsafe destination,
/// candidate import/reopen failure, or durable receipt failure.
pub fn import_stage(
    manifest_path: &Path,
    destination_root: &Path,
    accept_all: bool,
) -> ImportResult<ImportReceipt> {
    import_stage_inner(manifest_path, destination_root, accept_all, |_| {}, |_| {})
}

fn import_stage_inner(
    manifest_path: &Path,
    destination_root: &Path,
    accept_all: bool,
    before_create: impl FnOnce(&Path),
    after_incomplete: impl FnOnce(&Path),
) -> ImportResult<ImportReceipt> {
    let loaded = load_stage(manifest_path)?;
    if !accept_all {
        return Err(ImportError::AcceptanceRequired);
    }
    if !clean_absolute(destination_root) {
        return invalid("destination root must be absolute and clean");
    }
    let destination_identity = create_destination_root(destination_root, before_create)?;
    destination_identity.ensure_current(destination_root)?;
    write_incomplete(destination_root, loaded.report.manifest_sha256)?;
    destination_identity.ensure_current(destination_root)?;
    destination_identity.sync()?;
    after_incomplete(destination_root);

    let mut receipt_candidates = Vec::with_capacity(loaded.databases.len());
    for (index, (database, import)) in loaded.databases.into_iter().enumerate() {
        let file_name = format!("{index:04}-{}.redb", database.id);
        let path = destination_root.join(&file_name);
        let expected = JsonStageProvenance {
            stage_version: ptrack_store::JSON_STAGE_VERSION,
            source_format: import.source_format,
            batch_manifest_sha256: import.batch_manifest_sha256,
            database_json_sha256: import.database_json_sha256,
            quarantine_count: import.quarantine.len() as u64,
        };
        let kind = import.kind;
        destination_identity.ensure_current(destination_root)?;
        let (_, report) = Store::import_json_stage_new(&path, import)?;
        destination_identity.ensure_current(destination_root)?;
        let reopened = Store::open_existing(&path, kind)?;
        if reopened.json_stage_provenance()? != Some(expected)
            || report.quarantine_count != expected.quarantine_count
        {
            return invalid("candidate provenance failed close/reopen verification");
        }
        drop(reopened);
        let file_sha256 = hash_private_file(&path)?;
        destination_identity.ensure_current(destination_root)?;
        receipt_candidates.push(ReceiptCandidate {
            id: database.id,
            kind: match database.kind {
                DatabaseKind::Global => "global",
                DatabaseKind::Project => "project",
            },
            path: file_name,
            source_format: expected.source_format.to_string(),
            database_json_sha256: hex(expected.database_json_sha256),
            quarantine_count: expected.quarantine_count.to_string(),
            file_sha256,
        });
    }
    destination_identity.ensure_current(destination_root)?;
    write_receipt(destination_root, &loaded.report, &receipt_candidates)?;
    destination_identity.ensure_current(destination_root)?;
    destination_identity.sync()?;
    fs::remove_file(destination_root.join(INCOMPLETE_NAME))?;
    destination_identity.ensure_current(destination_root)?;
    destination_identity.sync()?;
    Ok(ImportReceipt {
        report: loaded.report,
        candidate_count: receipt_candidates.len() as u64,
    })
}

fn hash_private_file(path: &Path) -> ImportResult<String> {
    let mut file = ptrack_store::open_private_path(path, false, false)?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024].into_boxed_slice();
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

#[cfg(test)]
pub(super) fn import_stage_with_after_incomplete(
    manifest_path: &Path,
    destination_root: &Path,
    after_incomplete: impl FnOnce(&Path),
) -> ImportResult<ImportReceipt> {
    import_stage_inner(
        manifest_path,
        destination_root,
        true,
        |_| {},
        after_incomplete,
    )
}

#[cfg(test)]
pub(super) fn import_stage_with_before_create(
    manifest_path: &Path,
    destination_root: &Path,
    before_create: impl FnOnce(&Path),
) -> ImportResult<ImportReceipt> {
    import_stage_inner(manifest_path, destination_root, true, before_create, |_| {})
}

fn create_destination_root(
    path: &Path,
    before_create: impl FnOnce(&Path),
) -> ImportResult<DestinationIdentity> {
    if path.components().any(|component| {
        !matches!(
            component,
            Component::Prefix(_) | Component::RootDir | Component::Normal(_)
        )
    }) {
        return invalid("destination root is not lexically clean");
    }
    let parent_path = path.parent().ok_or_else(|| {
        ImportError::InvalidStage("destination root has no parent directory".to_owned())
    })?;
    let parent = ParentIdentity::capture(parent_path)?;
    before_create(parent_path);
    parent.ensure_current(parent_path)?;
    fs::create_dir(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            ImportError::InvalidStage("destination root must be absent".to_owned())
        } else {
            ImportError::Io(error)
        }
    })?;
    parent.ensure_current(parent_path)?;
    set_private_directory(path)?;
    parent.ensure_current(parent_path)?;
    let destination = DestinationIdentity::capture(path, parent)?;
    destination.parent.sync()?;
    destination.ensure_current(path)?;
    Ok(destination)
}

struct DestinationIdentity {
    parent: ParentIdentity,
    directory: File,
    identity: ptrack_store::PrivatePathIdentity,
}

impl DestinationIdentity {
    fn capture(path: &Path, parent: ParentIdentity) -> ImportResult<Self> {
        let identity = ptrack_store::verify_private_path(path, true)?;
        let directory = ptrack_store::open_private_path(path, true, true)?;
        if ptrack_store::verify_private_path(path, true)? != identity {
            return invalid("destination root changed while it was pinned");
        }
        Ok(Self {
            parent,
            directory,
            identity,
        })
    }

    fn ensure_current(&self, path: &Path) -> ImportResult<()> {
        self.parent.ensure_current(path.parent().ok_or_else(|| {
            ImportError::InvalidStage("destination root has no parent directory".to_owned())
        })?)?;
        if ptrack_store::verify_private_path(path, true)? != self.identity {
            return invalid("destination root identity changed during import");
        }
        Ok(())
    }

    fn sync(&self) -> ImportResult<()> {
        self.directory.sync_all()?;
        Ok(())
    }
}

struct ParentIdentity {
    directory: File,
    identity: ptrack_store::PrivatePathIdentity,
}

impl ParentIdentity {
    fn capture(path: &Path) -> ImportResult<Self> {
        let identity = ptrack_store::verify_private_path(path, true)?;
        let directory = ptrack_store::open_private_path(path, true, true)?;
        if ptrack_store::verify_private_path(path, true)? != identity {
            return invalid("destination parent changed while it was pinned");
        }
        Ok(Self {
            directory,
            identity,
        })
    }

    fn ensure_current(&self, path: &Path) -> ImportResult<()> {
        if ptrack_store::verify_private_path(path, true)? != self.identity {
            return invalid("destination parent identity changed during import");
        }
        Ok(())
    }

    fn sync(&self) -> ImportResult<()> {
        self.directory.sync_all()?;
        Ok(())
    }
}

fn write_receipt(
    root: &Path,
    report: &StageReport,
    candidates: &[ReceiptCandidate],
) -> ImportResult<()> {
    let receipt = Receipt {
        format: "ptrack-db-import-receipt",
        version: "1",
        manifest_sha256: hex(report.manifest_sha256),
        database_count: report.database_count.to_string(),
        quarantine_count: report.quarantine_count.to_string(),
        candidates,
    };
    let mut bytes = serde_json::to_vec(&receipt).map_err(|error| {
        ImportError::InvalidStage(format!("encode canonical import receipt: {error}"))
    })?;
    bytes.push(b'\n');
    let temporary = root.join(".receipt.json.tmp");
    let receipt_path = root.join(RECEIPT_NAME);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    set_private_file(&temporary, &file)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temporary, &receipt_path)?;
    Ok(())
}

fn write_incomplete(root: &Path, manifest_sha256: [u8; 32]) -> ImportResult<()> {
    #[derive(Serialize)]
    struct Incomplete {
        format: &'static str,
        version: &'static str,
        manifest_sha256: String,
        state: &'static str,
    }
    let mut bytes = serde_json::to_vec(&Incomplete {
        format: "ptrack-db-import-batch",
        version: "1",
        manifest_sha256: hex(manifest_sha256),
        state: "incomplete",
    })
    .map_err(|error| {
        ImportError::InvalidStage(format!("encode canonical incomplete marker: {error}"))
    })?;
    bytes.push(b'\n');
    let path = root.join(INCOMPLETE_NAME);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    set_private_file(&path, &file)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(())
}

fn set_private_directory(path: &Path) -> ImportResult<()> {
    ptrack_store::protect_private_directory(path)?;
    Ok(())
}

fn set_private_file(path: &Path, _: &File) -> ImportResult<()> {
    ptrack_store::protect_private_file(path)?;
    Ok(())
}
