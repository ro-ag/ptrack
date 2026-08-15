//! Read-only data and diagnostics aggregate for the Settings dialog.
//!
//! Every datum is read through an existing store or path API. A datum that is
//! genuinely unavailable is reported with an explicit status instead of a
//! fabricated value. The report never contains secrets, tokens, or capability
//! credentials.

use std::fs;
use std::path::{Path, PathBuf};

use ptrack_store::{GlobalStore, JsonStageProvenance, Store, StoreKind, StoreResult};
use serde::Serialize;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::desktop_runtime::WorkspaceProject;
use crate::{ActiveRuntime, AppResult};

const BACKUP_LEDGER_LIMIT: usize = 25;
const MIGRATION_RECEIPT_LIMIT: usize = 25;
const MIGRATIONS_DIRECTORY: &str = "migrations";
const RECEIPT_FILENAME: &str = "receipt.json";
const RUNTIME_DIRECTORY: &str = "runtime";
const UPDATES_DIRECTORY: &str = "updates";
const BACKUPS_DIRECTORY: &str = "backups";

/// Whether one aggregated datum could be read at all.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DatumStatusV1 {
    Available,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeStatusV1 {
    Active,
    Uninitialized,
    RecoveryRequired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MigrationDatabaseV1 {
    Global,
    Project,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsProjectPathsV1 {
    pub root: String,
    pub database: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsPathsV1 {
    pub global_home: String,
    /// The marker-attested global database, or `null` while the marker cannot
    /// be read.
    pub global_database: Option<String>,
    pub runtime_directory: String,
    pub updates_directory: String,
    pub backups_directory: String,
    pub migrations_directory: String,
    /// `null` while no project workspace is open.
    pub project: Option<DiagnosticsProjectPathsV1>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsRuntimeV1 {
    pub status: RuntimeStatusV1,
    /// Remediation text for a runtime that is not active.
    pub detail: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupLedgerEntryV1 {
    pub recorded_at: String,
    pub project: String,
    pub path: String,
    pub present: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupLedgerV1 {
    pub status: DatumStatusV1,
    pub entries: Vec<BackupLedgerEntryV1>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationQuarantineV1 {
    pub database: MigrationDatabaseV1,
    pub status: DatumStatusV1,
    pub count: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationDiagnosticsV1 {
    pub quarantine: Vec<MigrationQuarantineV1>,
    pub receipts: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityCountsV1 {
    pub granted: usize,
    pub total: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsReportV1 {
    pub paths: DiagnosticsPathsV1,
    pub runtime: DiagnosticsRuntimeV1,
    pub backups: BackupLedgerV1,
    pub migration: MigrationDiagnosticsV1,
    /// `null` while no project workspace is open to count grants against.
    pub capabilities: Option<CapabilityCountsV1>,
}

/// Aggregates the read-only diagnostics view. Never fails: an unreadable datum
/// is reported with its own status.
pub fn report(
    home: &Path,
    writer_version: &str,
    project: Option<&WorkspaceProject>,
    capabilities: Option<CapabilityCountsV1>,
) -> DiagnosticsReportV1 {
    let (status, detail, global) = match ActiveRuntime::load(home, writer_version) {
        Ok(Some(runtime)) => (
            RuntimeStatusV1::Active,
            String::new(),
            open_global(&runtime).ok(),
        ),
        Ok(None) => (
            RuntimeStatusV1::Uninitialized,
            "no p-track runtime is initialized (run 'ptrack init')".to_owned(),
            None,
        ),
        // Every active-generation load failure is a recovery-required state.
        Err(error) => (RuntimeStatusV1::RecoveryRequired, error.to_string(), None),
    };
    let store = global.as_ref().map(|(_, store)| store);
    DiagnosticsReportV1 {
        paths: DiagnosticsPathsV1 {
            global_home: display(home),
            global_database: global.as_ref().map(|(path, _)| display(path)),
            runtime_directory: display(&home.join(RUNTIME_DIRECTORY)),
            updates_directory: display(&home.join(UPDATES_DIRECTORY)),
            backups_directory: display(&home.join(BACKUPS_DIRECTORY)),
            migrations_directory: display(&home.join(MIGRATIONS_DIRECTORY)),
            project: project.map(|project| DiagnosticsProjectPathsV1 {
                root: project.root.clone(),
                database: project.db_path.clone(),
            }),
        },
        runtime: DiagnosticsRuntimeV1 { status, detail },
        backups: backups(store),
        migration: MigrationDiagnosticsV1 {
            quarantine: quarantine(store, project),
            receipts: receipts(&home.join(MIGRATIONS_DIRECTORY)),
        },
        capabilities,
    }
}

fn open_global(runtime: &ActiveRuntime) -> AppResult<(PathBuf, GlobalStore)> {
    let bindings = runtime.global_bindings(runtime.global_home())?;
    let store = GlobalStore::open_existing(&bindings.global_database, &bindings.global_binding)?;
    Ok((bindings.global_database, store))
}

fn backups(store: Option<&GlobalStore>) -> BackupLedgerV1 {
    let Some(rows) = store.and_then(|store| store.backups(BACKUP_LEDGER_LIMIT).ok()) else {
        return BackupLedgerV1 {
            status: DatumStatusV1::Unavailable,
            entries: Vec::new(),
        };
    };
    BackupLedgerV1 {
        status: DatumStatusV1::Available,
        entries: rows
            .into_iter()
            .map(|(recorded_at, project, path)| BackupLedgerEntryV1 {
                recorded_at: timestamp(recorded_at),
                project,
                present: Path::new(&path).is_file(),
                path,
            })
            .collect(),
    }
}

fn quarantine(
    store: Option<&GlobalStore>,
    project: Option<&WorkspaceProject>,
) -> Vec<MigrationQuarantineV1> {
    let mut rows = vec![row(
        MigrationDatabaseV1::Global,
        count(store.map(GlobalStore::json_stage_provenance)),
    )];
    if let Some(project) = project {
        rows.push(row(
            MigrationDatabaseV1::Project,
            count(
                Store::open_existing(&project.db_path, StoreKind::Project)
                    .ok()
                    .map(|store| store.json_stage_provenance()),
            ),
        ));
    }
    rows
}

/// No staged provenance means the store was never imported, so it holds no
/// quarantined legacy records. An unreadable store has no count at all.
fn count(provenance: Option<StoreResult<Option<JsonStageProvenance>>>) -> Option<u64> {
    provenance?
        .ok()
        .map(|provenance| provenance.map_or(0, |provenance| provenance.quarantine_count))
}

fn row(database: MigrationDatabaseV1, count: Option<u64>) -> MigrationQuarantineV1 {
    MigrationQuarantineV1 {
        database,
        status: if count.is_some() {
            DatumStatusV1::Available
        } else {
            DatumStatusV1::Unavailable
        },
        count,
    }
}

fn receipts(migrations: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(migrations) else {
        return Vec::new();
    };
    let mut receipts = entries
        .flatten()
        .map(|entry| entry.path().join(RECEIPT_FILENAME))
        .filter(|path| path.is_file())
        .map(|path| display(&path))
        .take(MIGRATION_RECEIPT_LIMIT)
        .collect::<Vec<_>>();
    receipts.sort();
    receipts
}

fn timestamp(nanoseconds: i64) -> String {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(nanoseconds))
        .ok()
        .and_then(|value| value.format(&Rfc3339).ok())
        .unwrap_or_default()
}

fn display(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
