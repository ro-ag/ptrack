use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use ptrack_store::GlobalStore;

use crate::diagnostics_report::{
    CapabilityCountsV1, DatumStatusV1, MigrationDatabaseV1, RuntimeStatusV1, report,
};
use crate::{ApplicationPort, InitRequest, RoutedApplication, WorkspaceProject};

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Temp(PathBuf);

impl Temp {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "ptrack-diagnostics-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        ptrack_store::protect_private_directory(&path).unwrap();
        Self(std::fs::canonicalize(path).unwrap())
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn an_uninitialized_home_reports_paths_with_explicitly_absent_data() {
    let directory = Temp::new("uninitialized");
    let home = directory.0.join("home");
    let report = report(&home, "test", None, None);

    assert_eq!(report.runtime.status, RuntimeStatusV1::Uninitialized);
    assert!(report.runtime.detail.contains("ptrack init"));
    assert_eq!(report.paths.global_home, home.to_string_lossy());
    assert_eq!(report.paths.global_database, None);
    assert_eq!(
        report.paths.runtime_directory,
        home.join("runtime").to_string_lossy()
    );
    assert_eq!(
        report.paths.updates_directory,
        home.join("updates").to_string_lossy()
    );
    assert_eq!(
        report.paths.backups_directory,
        home.join("backups").to_string_lossy()
    );
    assert_eq!(report.paths.project, None);
    assert_eq!(report.backups.status, DatumStatusV1::Unavailable);
    assert!(report.backups.entries.is_empty());
    assert_eq!(report.migration.quarantine.len(), 1);
    assert_eq!(report.migration.quarantine[0].count, None);
    assert_eq!(
        report.migration.quarantine[0].status,
        DatumStatusV1::Unavailable
    );
    assert!(report.migration.receipts.is_empty());
    assert_eq!(report.capabilities, None);
}

#[test]
fn an_initialized_home_reports_the_marker_ledger_receipts_and_quarantine_counts() {
    let directory = Temp::new("initialized");
    let home = directory.0.join("home");
    let project = directory.0.join("project");
    std::fs::create_dir(&home).unwrap();
    std::fs::create_dir(&project).unwrap();
    ptrack_store::protect_private_directory(&home).unwrap();

    let mut application = RoutedApplication::new(home.clone(), project.clone(), "test");
    application
        .initialize(InitRequest {
            root: Some(project.clone()),
            goal: String::new(),
            force: false,
            no_guide: true,
        })
        .unwrap();
    let runtime = application.active_runtime().unwrap().unwrap();
    let bindings = runtime.global_bindings(runtime.global_home()).unwrap();
    let database = project.join(".ptrack").join("ptrack.redb");
    let backup = home.join("backups").join("project-1.db");
    GlobalStore::open_existing(&bindings.global_database, &bindings.global_binding)
        .unwrap()
        .record_backup(1_700_000_000_000_000_000, &project, &backup)
        .unwrap();
    let batch = home.join("migrations").join("batch-a");
    std::fs::create_dir_all(&batch).unwrap();
    std::fs::write(batch.join("receipt.json"), b"{}").unwrap();

    let workspace = WorkspaceProject {
        name: "project".to_owned(),
        root: project.to_string_lossy().into_owned(),
        db_path: database.to_string_lossy().into_owned(),
    };
    let report = report(
        runtime.global_home(),
        "test",
        Some(&workspace),
        Some(CapabilityCountsV1 {
            granted: 1,
            total: 3,
        }),
    );

    assert_eq!(report.runtime.status, RuntimeStatusV1::Active);
    assert!(report.runtime.detail.is_empty());
    assert_eq!(
        report.paths.global_database.as_deref(),
        bindings.global_database.to_str()
    );
    assert_eq!(
        report
            .paths
            .project
            .as_ref()
            .map(|paths| paths.database.clone()),
        database.to_str().map(ToOwned::to_owned)
    );

    assert_eq!(report.backups.status, DatumStatusV1::Available);
    assert_eq!(report.backups.entries.len(), 1);
    let entry = &report.backups.entries[0];
    assert_eq!(entry.recorded_at, "2023-11-14T22:13:20Z");
    assert_eq!(entry.project, project.to_string_lossy());
    assert_eq!(entry.path, backup.to_string_lossy());
    assert!(!entry.present);

    // Natively created stores were never imported, so they quarantine nothing.
    assert_eq!(report.migration.quarantine.len(), 2);
    assert_eq!(
        report.migration.quarantine[0].database,
        MigrationDatabaseV1::Global
    );
    assert_eq!(report.migration.quarantine[0].count, Some(0));
    assert_eq!(
        report.migration.quarantine[1].database,
        MigrationDatabaseV1::Project
    );
    assert_eq!(report.migration.quarantine[1].count, Some(0));
    assert_eq!(
        report.migration.receipts,
        vec![batch.join("receipt.json").to_string_lossy().into_owned()]
    );
    assert_eq!(
        report.capabilities,
        Some(CapabilityCountsV1 {
            granted: 1,
            total: 3
        })
    );
}

#[test]
fn the_report_serializes_camel_case_wire_names() {
    let directory = Temp::new("wire");
    let value =
        serde_json::to_value(report(&directory.0.join("home"), "test", None, None)).unwrap();

    assert_eq!(value["runtime"]["status"], "uninitialized");
    assert_eq!(value["paths"]["globalDatabase"], serde_json::Value::Null);
    assert!(value["paths"]["migrationsDirectory"].is_string());
    assert_eq!(value["backups"]["status"], "unavailable");
    assert_eq!(value["migration"]["quarantine"][0]["database"], "global");
    assert_eq!(value["capabilities"], serde_json::Value::Null);
}
