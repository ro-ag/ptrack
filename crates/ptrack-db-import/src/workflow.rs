use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Component, Path};

use ptrack_store::{JsonStageProvenance, Store};
use serde::Serialize;

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

#[derive(Serialize)]
struct ReceiptCandidate {
    id: String,
    kind: &'static str,
    path: String,
    source_format: String,
    database_json_sha256: String,
    quarantine_count: String,
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
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

impl DestinationIdentity {
    #[cfg(unix)]
    fn capture(path: &Path, parent: ParentIdentity) -> ImportResult<Self> {
        use std::os::unix::fs::MetadataExt;

        let before = fs::symlink_metadata(path)?;
        if before.file_type().is_symlink() || !before.is_dir() {
            return invalid("destination root is not a real directory");
        }
        let directory = File::open(path)?;
        let opened = directory.metadata()?;
        let after = fs::symlink_metadata(path)?;
        if before.dev() != opened.dev()
            || before.ino() != opened.ino()
            || after.dev() != opened.dev()
            || after.ino() != opened.ino()
        {
            return invalid("destination root changed while it was pinned");
        }
        Ok(Self {
            parent,
            directory,
            device: opened.dev(),
            inode: opened.ino(),
        })
    }

    #[cfg(not(unix))]
    fn capture(_: &Path, _: ParentIdentity) -> ImportResult<Self> {
        invalid("safe destination identity pinning is not supported on this platform")
    }

    #[cfg(unix)]
    fn ensure_current(&self, path: &Path) -> ImportResult<()> {
        use std::os::unix::fs::MetadataExt;

        self.parent.ensure_current(path.parent().ok_or_else(|| {
            ImportError::InvalidStage("destination root has no parent directory".to_owned())
        })?)?;
        let current = fs::symlink_metadata(path)?;
        if current.file_type().is_symlink()
            || !current.is_dir()
            || current.dev() != self.device
            || current.ino() != self.inode
        {
            return invalid("destination root identity changed during import");
        }
        Ok(())
    }

    #[cfg(not(unix))]
    fn ensure_current(&self, _: &Path) -> ImportResult<()> {
        invalid("safe destination identity verification is not supported on this platform")
    }

    fn sync(&self) -> ImportResult<()> {
        self.directory.sync_all()?;
        Ok(())
    }
}

struct ParentIdentity {
    directory: File,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

impl ParentIdentity {
    #[cfg(unix)]
    fn capture(path: &Path) -> ImportResult<Self> {
        use std::os::unix::fs::MetadataExt;

        let before = fs::symlink_metadata(path)?;
        if before.file_type().is_symlink() || !before.is_dir() {
            return invalid("destination parent is not a real directory");
        }
        let directory = File::open(path)?;
        let opened = directory.metadata()?;
        let after = fs::symlink_metadata(path)?;
        if before.dev() != opened.dev()
            || before.ino() != opened.ino()
            || after.dev() != opened.dev()
            || after.ino() != opened.ino()
        {
            return invalid("destination parent changed while it was pinned");
        }
        Ok(Self {
            directory,
            device: opened.dev(),
            inode: opened.ino(),
        })
    }

    #[cfg(not(unix))]
    fn capture(_: &Path) -> ImportResult<Self> {
        invalid("safe destination parent pinning is not supported on this platform")
    }

    #[cfg(unix)]
    fn ensure_current(&self, path: &Path) -> ImportResult<()> {
        use std::os::unix::fs::MetadataExt;

        let current = fs::symlink_metadata(path)?;
        if current.file_type().is_symlink()
            || !current.is_dir()
            || current.dev() != self.device
            || current.ino() != self.inode
        {
            return invalid("destination parent identity changed during import");
        }
        Ok(())
    }

    #[cfg(not(unix))]
    fn ensure_current(&self, _: &Path) -> ImportResult<()> {
        invalid("safe destination parent verification is not supported on this platform")
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
    set_private_file(&file)?;
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
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    set_private_file(&file)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(unix)]
fn set_private_directory(path: &Path) -> ImportResult<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_directory(_: &Path) -> ImportResult<()> {
    Ok(())
}

#[cfg(unix)]
fn set_private_file(file: &File) -> ImportResult<()> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_file(_: &File) -> ImportResult<()> {
    Ok(())
}
