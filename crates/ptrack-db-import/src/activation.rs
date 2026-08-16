use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[cfg(windows)]
use ptrack_store::protect_private_file;
use ptrack_store::{
    ActiveBinding, ActiveGeneration, ActiveGenerationProject, CutoverLockMode, GlobalStore,
    JsonStageProvenance, LegacyReadLease, PrivatePathIdentity, ProjectStore, RetainedActiveStore,
    StagedStore, StoreKind, acquire_cutover_lock, acquire_legacy_read_lease,
    install_active_generation_retained, load_active_generation, open_private_path,
    replace_private_file, restore_active_generation, sync_private_directory,
    verify_legacy_source_identity, verify_private_open_handle, verify_private_path,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{ImportError, ImportResult, invalid};
use crate::manifest::{DatabaseEntry, DatabaseKind, SourceIdentity, clean_absolute};
use crate::sha256::hex;
use crate::stage::{LoadedStage, load_stage};

const FORMAT_VERSION: &str = "1";
const MAX_RECORD_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActivationReceipt {
    pub format: String,
    pub version: String,
    pub state: String,
    pub batch_id: String,
    pub generation: String,
    pub plan_sha256: String,
    pub handoff_sha256: String,
    pub journal_sha256: String,
    pub marker_sha256: String,
    pub marker: ActiveGeneration,
    pub previous_marker: Option<ActiveGeneration>,
    pub destinations: Vec<ActivationDestination>,
    pub installed_destinations: Vec<InstalledDestinationReceipt>,
    pub legacy_sources: Vec<LegacySourceReceipt>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActivationDestination {
    pub database_id: String,
    pub kind: String,
    pub path: String,
    pub candidate_sha256: String,
    pub source_format: String,
    pub database_json_sha256: String,
    pub record_count: String,
    pub quarantine_count: String,
    pub collection_state_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstalledDestinationReceipt {
    pub database_id: String,
    pub path: String,
    pub device: String,
    pub inode: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LegacySourceReceipt {
    pub path: String,
    pub device: String,
    pub inode: String,
    pub size: String,
    pub mtime_seconds: String,
    pub mtime_nanos: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ActivationPlan {
    format: String,
    version: String,
    batch_id: String,
    generation: String,
    manifest_sha256: String,
    previous_marker: Option<ActiveGeneration>,
    destinations: Vec<ActivationDestination>,
    legacy_sources: Vec<LegacySourceReceipt>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ActivationJournal {
    format: String,
    version: String,
    sequence: String,
    state: String,
    predecessor: String,
    plan_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ActivationHandoff {
    format: String,
    version: String,
    state: String,
    plan_sha256: String,
    journal_sha256: String,
}

struct InstallSpec {
    database: DatabaseEntry,
    candidate: PathBuf,
    destination: PathBuf,
    kind: StoreKind,
    candidate_sha256: String,
    source_format: u64,
    database_json_sha256: String,
    record_count: u64,
    quarantine_count: u64,
    collection_state_sha256: String,
    expected_provenance: JsonStageProvenance,
}

struct DestinationFence {
    path: PathBuf,
    identity: PrivatePathIdentity,
    file: File,
    parent: DestinationParentFence,
    store: DestinationStoreFence,
}

enum DestinationStoreFence {
    Global(GlobalStore),
    Project(ProjectStore),
}

struct DestinationParentFence {
    path: PathBuf,
    identity: PrivatePathIdentity,
    file: File,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CandidateImportReceipt {
    format: String,
    version: String,
    manifest_sha256: String,
    database_count: String,
    quarantine_count: String,
    candidates: Vec<CandidateReceipt>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CandidateReceipt {
    id: String,
    kind: String,
    path: String,
    source_format: String,
    database_json_sha256: String,
    quarantine_count: String,
    file_sha256: String,
}

/// Installs and activates one previously verified import batch. The operation
/// is offline, exclusive, durable, resumable, and never deletes legacy data.
///
/// # Errors
/// Returns an error for unsafe paths, missing acceptance, source drift, lock
/// contention, candidate/store mismatch, durability failure, or inconsistent
/// resume state.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub fn activate_stage(
    manifest_path: &Path,
    candidates_root: &Path,
    batch_root: &Path,
    global_home: &Path,
    generation: u64,
    writer_version: &str,
    accept_all: bool,
) -> ImportResult<ActivationReceipt> {
    if !accept_all {
        return Err(ImportError::AcceptanceRequired);
    }
    if generation == 0 || writer_version.is_empty() {
        return invalid("activation generation and writer version are required");
    }
    validate_activation_paths(manifest_path, candidates_root, batch_root, global_home)?;
    validate_private_directory(batch_root)?;
    validate_private_directory(candidates_root)?;
    let loaded = load_stage(manifest_path)?;
    let batch_id = batch_root
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| ImportError::InvalidStage("batch ID is invalid".to_owned()))?;
    validate_id(batch_id, "batch ID")?;
    let candidate_hashes = require_candidate_receipt(candidates_root, &loaded)?;

    let home = fs::canonicalize(global_home)?;
    let lease = acquire_cutover_lock(&home, CutoverLockMode::Exclusive)?;
    let fences = acquire_legacy_fences(&loaded)?;
    let observed_marker = load_active_generation(&home, &lease)?;
    let specs = install_specs(&loaded, candidates_root, &home, &candidate_hashes)?;
    let marker = marker_for(generation, &home, &specs)?;
    let destinations = specs
        .iter()
        .map(|spec| ActivationDestination {
            database_id: spec.database.id.clone(),
            kind: kind_name(spec.kind).to_owned(),
            path: spec.destination.to_string_lossy().into_owned(),
            candidate_sha256: spec.candidate_sha256.clone(),
            source_format: spec.source_format.to_string(),
            database_json_sha256: spec.database_json_sha256.clone(),
            record_count: spec.record_count.to_string(),
            quarantine_count: spec.quarantine_count.to_string(),
            collection_state_sha256: spec.collection_state_sha256.clone(),
        })
        .collect::<Vec<_>>();
    let legacy_sources = fences
        .iter()
        .map(|fence| LegacySourceReceipt {
            path: fence.path.to_string_lossy().into_owned(),
            device: fence.identity.device.clone(),
            inode: fence.identity.inode.clone(),
            size: fence.identity.size.clone(),
            mtime_seconds: fence.identity.mtime_seconds.clone(),
            mtime_nanos: fence.identity.mtime_nanos.clone(),
            sha256: fence.sha256.clone(),
        })
        .collect::<Vec<_>>();
    let proposed_plan = ActivationPlan {
        format: "ptrack-cutover-plan".to_owned(),
        version: FORMAT_VERSION.to_owned(),
        batch_id: batch_id.to_owned(),
        generation: generation.to_string(),
        manifest_sha256: hex(loaded.report.manifest_sha256),
        previous_marker: observed_marker.clone(),
        destinations: destinations.clone(),
        legacy_sources: legacy_sources.clone(),
    };
    let plan_path = batch_root.join("plan.json");
    let plan = if plan_path.exists() {
        let plan: ActivationPlan = read_canonical(&plan_path)?;
        validate_plan(
            &plan,
            batch_id,
            generation,
            &hex(loaded.report.manifest_sha256),
            &destinations,
            &legacy_sources,
        )?;
        plan
    } else {
        publish_new(&plan_path, &canonical_bytes(&proposed_plan)?)?;
        proposed_plan
    };
    if observed_marker != plan.previous_marker && observed_marker.as_ref() != Some(&marker) {
        return invalid("active-generation marker does not match the activation plan");
    }
    let plan_bytes = canonical_bytes(&plan)?;
    let plan_sha256 = sha256_hex(&plan_bytes);
    let previous_marker = plan.previous_marker.clone();

    let receipt_path = batch_root.join("receipt.json");
    if receipt_path.exists() {
        let receipt: ActivationReceipt = read_canonical(&receipt_path)?;
        let journal: ActivationJournal = read_canonical(&batch_root.join("journal.json"))?;
        validate_journal(&journal, &plan_sha256)?;
        let handoff_bytes = read_bounded(&batch_root.join("handoff.json"))?;
        let handoff: ActivationHandoff = serde_json::from_slice(&handoff_bytes)
            .map_err(|_| ImportError::InvalidStage("activation handoff is invalid".to_owned()))?;
        validate_handoff(&handoff, &plan_sha256)?;
        let installed_destinations = installed_destination_receipts(&specs)?;
        validate_receipt(
            &receipt,
            batch_id,
            generation,
            &plan_sha256,
            &sha256_hex(&handoff_bytes),
            &sha256_hex(&canonical_bytes(&journal)?),
            &destinations,
            &installed_destinations,
            &legacy_sources,
        )?;
        if journal.state != "marker-installed" {
            return invalid("activation receipt has no marker-installed journal");
        }
        if load_active_generation(&home, &lease)?.as_ref() != Some(&receipt.marker) {
            return invalid("activation receipt does not match the current marker");
        }
        revalidate_fences(&fences)?;
        return Ok(receipt);
    }

    let journal_path = batch_root.join("journal.json");
    let mut journal = if journal_path.exists() {
        let journal: ActivationJournal = read_canonical(&journal_path)?;
        validate_journal(&journal, &plan_sha256)?;
        journal
    } else {
        let journal = journal_record("1", "planned", "", &plan_sha256);
        publish_replace(&journal_path, &canonical_bytes(&journal)?)?;
        journal
    };

    let destination_fences = if journal.state == "planned" {
        let destination_fences = install_and_activate(&specs, generation, writer_version)?;
        revalidate_fences(&fences)?;
        journal = journal_record("2", "stores-installed", "planned", &plan_sha256);
        publish_replace(&journal_path, &canonical_bytes(&journal)?)?;
        destination_fences
    } else {
        capture_destination_fences(&specs, generation, writer_version)?
    };
    revalidate_destination_fences(&destination_fences)?;

    let handoff_path = batch_root.join("handoff.json");
    let installed_journal = journal_record("2", "stores-installed", "planned", &plan_sha256);
    let handoff = ActivationHandoff {
        format: "ptrack-cutover-handoff".to_owned(),
        version: FORMAT_VERSION.to_owned(),
        state: "READY_FOR_CUTOVER".to_owned(),
        plan_sha256: plan_sha256.clone(),
        journal_sha256: sha256_hex(&canonical_bytes(&installed_journal)?),
    };
    let handoff_bytes = canonical_bytes(&handoff)?;
    publish_immutable_or_verify(&handoff_path, &handoff_bytes)?;
    revalidate_destination_fences(&destination_fences)?;
    let handoff_sha256 = sha256_hex(&handoff_bytes);

    if journal.state == "stores-installed" {
        let current = load_active_generation(&home, &lease)?;
        if current.as_ref() == Some(&marker) {
            // Marker publication completed before the journal update.
        } else if current == previous_marker {
            let retained = destination_fences
                .iter()
                .map(|fence| match &fence.store {
                    DestinationStoreFence::Global(store) => RetainedActiveStore::Global(store),
                    DestinationStoreFence::Project(store) => RetainedActiveStore::Project(store),
                })
                .collect::<Vec<_>>();
            install_active_generation_retained(&home, &lease, &marker, &retained)?;
        } else {
            return invalid("active-generation marker changed during activation");
        }
        revalidate_destination_fences(&destination_fences)?;
        revalidate_fences(&fences)?;
        revalidate_destination_fences(&destination_fences)?;
        journal = journal_record("3", "marker-installed", "stores-installed", &plan_sha256);
        publish_replace(&journal_path, &canonical_bytes(&journal)?)?;
    } else if journal.state != "marker-installed" {
        return invalid("activation journal has an unsupported state");
    }

    if load_active_generation(&home, &lease)?.as_ref() != Some(&marker) {
        return invalid("installed marker failed close/reopen verification");
    }
    let marker_sha256 = sha256_hex(&canonical_bytes(&marker)?);
    let installed_destinations = installed_destination_receipts(&specs)?;
    let receipt = ActivationReceipt {
        format: "ptrack-cutover-receipt".to_owned(),
        version: FORMAT_VERSION.to_owned(),
        state: "ACTIVE".to_owned(),
        batch_id: batch_id.to_owned(),
        generation: generation.to_string(),
        plan_sha256,
        handoff_sha256,
        journal_sha256: sha256_hex(&canonical_bytes(&journal)?),
        marker_sha256,
        marker,
        previous_marker,
        destinations,
        installed_destinations,
        legacy_sources,
    };
    publish_immutable_or_verify(&receipt_path, &canonical_bytes(&receipt)?)?;
    revalidate_destination_fences(&destination_fences)?;
    revalidate_fences(&fences)?;
    Ok(receipt)
}

/// Restores the marker recorded by an activation receipt only while every
/// destination's application-write fuse remains false.
///
/// # Errors
/// Returns an error for an inconsistent receipt/current marker, any committed
/// application write, store/source drift, or unavailable exclusive fence.
pub fn rollback_activation(
    batch_root: &Path,
    global_home: &Path,
    writer_version: &str,
    accept_all: bool,
) -> ImportResult<()> {
    if !accept_all {
        return Err(ImportError::AcceptanceRequired);
    }
    validate_private_directory(batch_root)?;
    let receipt: ActivationReceipt = read_canonical(&batch_root.join("receipt.json"))?;
    if receipt.format != "ptrack-cutover-receipt"
        || receipt.version != FORMAT_VERSION
        || receipt.state != "ACTIVE"
        || receipt.marker_sha256 != sha256_hex(&canonical_bytes(&receipt.marker)?)
    {
        return invalid("activation receipt is invalid");
    }
    let plan_bytes = read_bounded(&batch_root.join("plan.json"))?;
    let plan: ActivationPlan = serde_json::from_slice(&plan_bytes)
        .map_err(|_| ImportError::InvalidStage("activation plan is invalid".to_owned()))?;
    if plan.format != "ptrack-cutover-plan"
        || plan.version != FORMAT_VERSION
        || receipt.plan_sha256 != sha256_hex(&plan_bytes)
        || receipt.previous_marker != plan.previous_marker
        || receipt.destinations != plan.destinations
        || receipt.legacy_sources != plan.legacy_sources
    {
        return invalid("activation receipt does not match its plan");
    }
    let journal: ActivationJournal = read_canonical(&batch_root.join("journal.json"))?;
    validate_journal(&journal, &receipt.plan_sha256)?;
    let handoff: ActivationHandoff = read_canonical(&batch_root.join("handoff.json"))?;
    validate_handoff(&handoff, &receipt.plan_sha256)?;
    if receipt.handoff_sha256 != sha256_hex(&canonical_bytes(&handoff)?)
        || (journal.state == "marker-installed"
            && receipt.journal_sha256 != sha256_hex(&canonical_bytes(&journal)?))
    {
        return invalid("activation receipt digest chain is inconsistent");
    }
    let home = fs::canonicalize(global_home)?;
    let lease = acquire_cutover_lock(&home, CutoverLockMode::Exclusive)?;
    let legacy_fences = acquire_receipt_legacy_fences(&receipt.legacy_sources)?;
    validate_installed_destination_receipts(&receipt)?;
    let current_marker = load_active_generation(&home, &lease)?;
    if journal.state == "rolled-back" {
        if current_marker.as_ref() != receipt.previous_marker.as_ref() {
            return invalid("rolled-back marker does not match the activation receipt");
        }
        return Ok(());
    }
    if journal.state != "marker-installed" {
        return invalid("rollback requires a marker-installed journal");
    }
    if current_marker.as_ref() == receipt.previous_marker.as_ref() {
        let rolled_back =
            journal_record("4", "rolled-back", "marker-installed", &receipt.plan_sha256);
        publish_replace(
            &batch_root.join("journal.json"),
            &canonical_bytes(&rolled_back)?,
        )?;
        revalidate_fences(&legacy_fences)?;
        return Ok(());
    }
    if current_marker.as_ref() != Some(&receipt.marker) {
        return invalid("current marker does not match the activation receipt");
    }
    for destination in &receipt.destinations {
        let binding = binding_from_destination(&receipt, destination)?;
        match binding.kind {
            StoreKind::Global => {
                let store = GlobalStore::open_existing(&destination.path, &binding)?;
                if store.application_writes()? {
                    return invalid("rollback is forbidden after an application write");
                }
            }
            StoreKind::Project => {
                let store =
                    ProjectStore::open_existing(&destination.path, &binding, writer_version)?;
                if store.application_writes()? {
                    return invalid("rollback is forbidden after an application write");
                }
            }
        }
    }
    restore_active_generation(
        &home,
        &lease,
        receipt.previous_marker.as_ref(),
        writer_version,
    )?;
    revalidate_fences(&legacy_fences)?;
    let rolled_back = journal_record("4", "rolled-back", "marker-installed", &receipt.plan_sha256);
    publish_replace(
        &batch_root.join("journal.json"),
        &canonical_bytes(&rolled_back)?,
    )?;
    Ok(())
}

fn install_specs(
    loaded: &LoadedStage,
    candidates_root: &Path,
    global_home: &Path,
    candidate_hashes: &BTreeMap<String, String>,
) -> ImportResult<Vec<InstallSpec>> {
    loaded
        .databases
        .iter()
        .enumerate()
        .map(|(index, (database, import))| {
            let candidate = candidates_root.join(format!("{index:04}-{}.redb", database.id));
            let (kind, destination) = match database.kind {
                DatabaseKind::Global => (StoreKind::Global, global_home.join("global.redb")),
                DatabaseKind::Project => {
                    let root = database.project_root.as_ref().ok_or_else(|| {
                        ImportError::InvalidStage("project root is missing".to_owned())
                    })?;
                    (
                        StoreKind::Project,
                        Path::new(root).join(".ptrack/ptrack.redb"),
                    )
                }
            };
            if import.kind != kind {
                return invalid("candidate kind does not match manifest");
            }
            Ok(InstallSpec {
                database: database.clone(),
                candidate,
                destination,
                kind,
                candidate_sha256: candidate_hashes
                    .get(&format!("{index:04}-{}.redb", database.id))
                    .cloned()
                    .ok_or_else(|| {
                        ImportError::InvalidStage("candidate receipt entry is missing".to_owned())
                    })?,
                source_format: import.source_format,
                database_json_sha256: hex(import.database_json_sha256),
                record_count: import
                    .collections
                    .iter()
                    .try_fold(0_u64, |total, collection| {
                        total
                            .checked_add(collection.records.len() as u64)
                            .ok_or_else(|| {
                                ImportError::InvalidStage(
                                    "candidate record count overflow".to_owned(),
                                )
                            })
                    })?,
                quarantine_count: import.quarantine.len() as u64,
                collection_state_sha256: collection_state_sha256(import)?,
                expected_provenance: JsonStageProvenance {
                    stage_version: ptrack_store::JSON_STAGE_VERSION,
                    source_format: import.source_format,
                    batch_manifest_sha256: import.batch_manifest_sha256,
                    database_json_sha256: import.database_json_sha256,
                    quarantine_count: import.quarantine.len() as u64,
                },
            })
        })
        .collect()
}

fn marker_for(
    generation: u64,
    global_home: &Path,
    specs: &[InstallSpec],
) -> ImportResult<ActiveGeneration> {
    let global = specs
        .iter()
        .find(|spec| spec.kind == StoreKind::Global)
        .ok_or_else(|| ImportError::InvalidStage("global candidate is missing".to_owned()))?;
    let mut projects = specs
        .iter()
        .filter(|spec| spec.kind == StoreKind::Project)
        .map(|spec| ActiveGenerationProject {
            root: spec.database.project_root.clone().unwrap_or_default(),
            database_id: spec.database.id.clone(),
            path: spec.destination.to_string_lossy().into_owned(),
        })
        .collect::<Vec<_>>();
    projects.sort_by(|left, right| left.root.cmp(&right.root));
    Ok(ActiveGeneration::new(
        generation,
        global.database.id.clone(),
        &global_home.join("global.redb"),
        projects,
    )?)
}

fn collection_state_sha256(import: &ptrack_store::JsonStageImportData) -> ImportResult<String> {
    let state = import
        .collections
        .iter()
        .map(|collection| {
            serde_json::json!({
                "name": collection.collection.name(),
                "sequence": collection.sequence.map(|value| value.to_string()),
                "record_count": collection.records.len().to_string()
            })
        })
        .collect::<Vec<_>>();
    Ok(sha256_hex(&serde_json::to_vec(&state).map_err(
        |error| ImportError::InvalidStage(format!("encode collection evidence: {error}")),
    )?))
}

fn installed_destination_receipts(
    specs: &[InstallSpec],
) -> ImportResult<Vec<InstalledDestinationReceipt>> {
    specs
        .iter()
        .map(|spec| {
            let identity = verify_private_path(&spec.destination, false)?;
            Ok(InstalledDestinationReceipt {
                database_id: spec.database.id.clone(),
                path: spec.destination.to_string_lossy().into_owned(),
                device: identity.device.to_string(),
                inode: identity.inode.to_string(),
            })
        })
        .collect()
}

fn validate_installed_destination_receipts(receipt: &ActivationReceipt) -> ImportResult<()> {
    if receipt.installed_destinations.len() != receipt.destinations.len() {
        return invalid("activation receipt destination identity count is inconsistent");
    }
    for (planned, installed) in receipt
        .destinations
        .iter()
        .zip(&receipt.installed_destinations)
    {
        let identity = verify_private_path(Path::new(&installed.path), false)?;
        if installed.database_id != planned.database_id
            || installed.path != planned.path
            || installed.device != identity.device.to_string()
            || installed.inode != identity.inode.to_string()
        {
            return invalid("installed destination identity changed after activation");
        }
    }
    Ok(())
}

fn install_and_activate(
    specs: &[InstallSpec],
    generation: u64,
    writer_version: &str,
) -> ImportResult<Vec<DestinationFence>> {
    let mut fences = Vec::with_capacity(specs.len());
    for spec in specs {
        if !spec.destination.exists() {
            copy_candidate(&spec.candidate, &spec.destination, &spec.candidate_sha256)?;
        }
        let binding = ActiveBinding {
            generation,
            database_id: spec.database.id.clone(),
            kind: spec.kind,
            canonical_path: spec.destination.clone(),
        };
        let destination_sha256 = hash_private_file(&spec.destination)?;
        let store = if destination_sha256 == spec.candidate_sha256 {
            let staged = StagedStore::open(&spec.destination, spec.kind).map_err(|_| {
                ImportError::InvalidStage(
                    "planned candidate destination is not an inactive staged store".to_owned(),
                )
            })?;
            if staged.provenance()? != spec.expected_provenance {
                return invalid("destination candidate provenance does not match the plan");
            }
            match spec.kind {
                StoreKind::Global => {
                    DestinationStoreFence::Global(GlobalStore::activate(staged, binding)?)
                }
                StoreKind::Project => DestinationStoreFence::Project(ProjectStore::activate(
                    staged,
                    binding,
                    writer_version,
                )?),
            }
        } else {
            open_destination_store(spec, &binding, writer_version)?
        };
        fences.push(capture_destination_fence(spec, store)?);
    }
    revalidate_destination_fences(&fences)?;
    Ok(fences)
}

fn capture_destination_fences(
    specs: &[InstallSpec],
    generation: u64,
    writer_version: &str,
) -> ImportResult<Vec<DestinationFence>> {
    specs
        .iter()
        .map(|spec| {
            let binding = ActiveBinding {
                generation,
                database_id: spec.database.id.clone(),
                kind: spec.kind,
                canonical_path: spec.destination.clone(),
            };
            let store = open_destination_store(spec, &binding, writer_version)?;
            capture_destination_fence(spec, store)
        })
        .collect()
}

fn open_destination_store(
    spec: &InstallSpec,
    binding: &ActiveBinding,
    writer_version: &str,
) -> ImportResult<DestinationStoreFence> {
    Ok(match spec.kind {
        StoreKind::Global => {
            DestinationStoreFence::Global(GlobalStore::open_existing(&spec.destination, binding)?)
        }
        StoreKind::Project => DestinationStoreFence::Project(ProjectStore::open_existing(
            &spec.destination,
            binding,
            writer_version,
        )?),
    })
}

fn capture_destination_fence(
    spec: &InstallSpec,
    store: DestinationStoreFence,
) -> ImportResult<DestinationFence> {
    let file = open_private_path(&spec.destination, false, false)?;
    let identity = verify_private_open_handle(&file)?;
    if verify_private_path(&spec.destination, false)? != identity {
        return invalid("installed destination changed while its fence was opened");
    }
    let parent_path = spec
        .destination
        .parent()
        .ok_or_else(|| ImportError::InvalidStage("destination has no parent".to_owned()))?;
    let parent = open_destination_parent_fence(parent_path)?;
    Ok(DestinationFence {
        path: spec.destination.clone(),
        identity,
        file,
        parent,
        store,
    })
}

fn revalidate_destination_fences(fences: &[DestinationFence]) -> ImportResult<()> {
    for fence in fences {
        if verify_private_open_handle(&fence.file)? != fence.identity
            || verify_private_path(&fence.path, false)? != fence.identity
        {
            return invalid("installed destination identity changed during activation");
        }
        let application_writes = match &fence.store {
            DestinationStoreFence::Global(store) => store.application_writes()?,
            DestinationStoreFence::Project(store) => store.application_writes()?,
        };
        if application_writes {
            return invalid("installed destination gained an application write during activation");
        }
        revalidate_destination_parent_fence(&fence.parent)?;
    }
    Ok(())
}

#[cfg(unix)]
fn open_destination_parent_fence(path: &Path) -> ImportResult<DestinationParentFence> {
    use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};

    let current = fs::symlink_metadata(path)?;
    if current.file_type().is_symlink() || !current.is_dir() {
        return invalid("destination parent must be a real directory");
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits().cast_signed())
        .open(path)?;
    let opened = file.metadata()?;
    if current.dev() != opened.dev() || current.ino() != opened.ino() {
        return invalid("destination parent changed while it was opened");
    }
    Ok(DestinationParentFence {
        path: path.to_path_buf(),
        identity: PrivatePathIdentity {
            device: current.dev(),
            inode: current.ino(),
        },
        file,
    })
}

#[cfg(windows)]
fn open_destination_parent_fence(path: &Path) -> ImportResult<DestinationParentFence> {
    let file = open_private_path(path, true, false)?;
    Ok(DestinationParentFence {
        path: path.to_path_buf(),
        identity: verify_private_path(path, true)?,
        file,
    })
}

#[cfg(not(any(unix, windows)))]
fn open_destination_parent_fence(_: &Path) -> ImportResult<DestinationParentFence> {
    invalid("destination parent verification is unsupported")
}

#[cfg(unix)]
fn revalidate_destination_parent_fence(fence: &DestinationParentFence) -> ImportResult<()> {
    use std::os::unix::fs::MetadataExt as _;

    let current = fs::symlink_metadata(&fence.path)?;
    let opened = fence.file.metadata()?;
    if current.file_type().is_symlink()
        || !current.is_dir()
        || current.dev() != fence.identity.device
        || current.ino() != fence.identity.inode
        || opened.dev() != fence.identity.device
        || opened.ino() != fence.identity.inode
    {
        return invalid("destination parent identity changed during activation");
    }
    Ok(())
}

#[cfg(windows)]
fn revalidate_destination_parent_fence(fence: &DestinationParentFence) -> ImportResult<()> {
    let _ = fence.file.metadata()?;
    if verify_private_path(&fence.path, true)? != fence.identity {
        return invalid("destination parent identity changed during activation");
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn revalidate_destination_parent_fence(_: &DestinationParentFence) -> ImportResult<()> {
    invalid("destination parent verification is unsupported")
}

fn binding_from_destination(
    receipt: &ActivationReceipt,
    destination: &ActivationDestination,
) -> ImportResult<ActiveBinding> {
    let generation = destination_generation(receipt)?;
    let kind = match destination.kind.as_str() {
        "global" => StoreKind::Global,
        "project" => StoreKind::Project,
        _ => return invalid("receipt destination kind is invalid"),
    };
    Ok(ActiveBinding {
        generation,
        database_id: destination.database_id.clone(),
        kind,
        canonical_path: PathBuf::from(&destination.path),
    })
}

fn destination_generation(receipt: &ActivationReceipt) -> ImportResult<u64> {
    receipt
        .generation
        .parse::<u64>()
        .ok()
        .filter(|value| *value != 0 && value.to_string() == receipt.generation)
        .ok_or_else(|| ImportError::InvalidStage("receipt generation is invalid".to_owned()))
}

#[allow(clippy::too_many_arguments)]
fn validate_receipt(
    receipt: &ActivationReceipt,
    batch_id: &str,
    generation: u64,
    plan_sha256: &str,
    handoff_sha256: &str,
    journal_sha256: &str,
    destinations: &[ActivationDestination],
    installed_destinations: &[InstalledDestinationReceipt],
    legacy_sources: &[LegacySourceReceipt],
) -> ImportResult<()> {
    if receipt.format != "ptrack-cutover-receipt"
        || receipt.version != FORMAT_VERSION
        || receipt.state != "ACTIVE"
        || receipt.batch_id != batch_id
        || receipt.generation != generation.to_string()
        || receipt.plan_sha256 != plan_sha256
        || receipt.handoff_sha256 != handoff_sha256
        || receipt.journal_sha256 != journal_sha256
        || receipt.destinations != destinations
        || receipt.installed_destinations != installed_destinations
        || receipt.legacy_sources != legacy_sources
        || receipt.marker_sha256 != sha256_hex(&canonical_bytes(&receipt.marker)?)
    {
        return invalid("activation receipt is inconsistent");
    }
    Ok(())
}

fn validate_plan(
    plan: &ActivationPlan,
    batch_id: &str,
    generation: u64,
    manifest_sha256: &str,
    destinations: &[ActivationDestination],
    legacy_sources: &[LegacySourceReceipt],
) -> ImportResult<()> {
    if plan.format != "ptrack-cutover-plan"
        || plan.version != FORMAT_VERSION
        || plan.batch_id != batch_id
        || plan.generation != generation.to_string()
        || plan.manifest_sha256 != manifest_sha256
        || plan.destinations != destinations
        || plan.legacy_sources != legacy_sources
    {
        return invalid("activation plan is inconsistent");
    }
    Ok(())
}

fn validate_handoff(handoff: &ActivationHandoff, plan_sha256: &str) -> ImportResult<()> {
    let installed = journal_record("2", "stores-installed", "planned", plan_sha256);
    if handoff.format != "ptrack-cutover-handoff"
        || handoff.version != FORMAT_VERSION
        || handoff.state != "READY_FOR_CUTOVER"
        || handoff.plan_sha256 != plan_sha256
        || handoff.journal_sha256 != sha256_hex(&canonical_bytes(&installed)?)
    {
        return invalid("activation handoff is inconsistent");
    }
    Ok(())
}

fn journal_record(
    sequence: &str,
    state: &str,
    predecessor: &str,
    plan_sha256: &str,
) -> ActivationJournal {
    ActivationJournal {
        format: "ptrack-cutover-journal".to_owned(),
        version: FORMAT_VERSION.to_owned(),
        sequence: sequence.to_owned(),
        state: state.to_owned(),
        predecessor: predecessor.to_owned(),
        plan_sha256: plan_sha256.to_owned(),
    }
}

fn validate_journal(journal: &ActivationJournal, plan_sha256: &str) -> ImportResult<()> {
    let valid_transition = matches!(
        (
            journal.sequence.as_str(),
            journal.state.as_str(),
            journal.predecessor.as_str()
        ),
        ("1", "planned", "")
            | ("2", "stores-installed", "planned")
            | ("3", "marker-installed", "stores-installed")
            | ("4", "rolled-back", "marker-installed")
    );
    if journal.format != "ptrack-cutover-journal"
        || journal.version != FORMAT_VERSION
        || journal.plan_sha256 != plan_sha256
        || !valid_transition
    {
        return invalid("activation journal transition is invalid");
    }
    Ok(())
}

fn validate_activation_paths(
    manifest_path: &Path,
    candidates_root: &Path,
    batch_root: &Path,
    global_home: &Path,
) -> ImportResult<()> {
    for path in [manifest_path, candidates_root, batch_root, global_home] {
        if !clean_absolute(path) {
            return invalid("activation paths must be absolute and clean");
        }
    }
    if candidates_root != batch_root.join("candidates") {
        return invalid("candidate root must be <batch>/candidates");
    }
    Ok(())
}

fn validate_id(value: &str, label: &str) -> ImportResult<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return invalid(format!("{label} is invalid"));
    }
    Ok(())
}

fn require_candidate_receipt(
    root: &Path,
    loaded: &LoadedStage,
) -> ImportResult<BTreeMap<String, String>> {
    let path = root.join("receipt.json");
    let receipt: CandidateImportReceipt = read_canonical(&path)?;
    if receipt.format != "ptrack-db-import-receipt"
        || receipt.version != FORMAT_VERSION
        || receipt.manifest_sha256 != hex(loaded.report.manifest_sha256)
        || receipt.database_count != loaded.report.database_count.to_string()
        || receipt.quarantine_count != loaded.report.quarantine_count.to_string()
        || receipt.candidates.len() != loaded.databases.len()
    {
        return invalid("candidate import receipt is missing or invalid");
    }
    let mut hashes = BTreeMap::new();
    for (index, (candidate, (database, import))) in
        receipt.candidates.iter().zip(&loaded.databases).enumerate()
    {
        let file_name = format!("{index:04}-{}.redb", database.id);
        let kind = kind_name(import.kind);
        if candidate.id != database.id
            || candidate.kind != kind
            || candidate.path != file_name
            || candidate.source_format != import.source_format.to_string()
            || candidate.database_json_sha256 != hex(import.database_json_sha256)
            || candidate.quarantine_count != import.quarantine.len().to_string()
            || candidate.file_sha256.len() != 64
            || !candidate
                .file_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return invalid("candidate import receipt does not match the validated stage");
        }
        let candidate_path = root.join(&file_name);
        if hash_private_file(&candidate_path)? != candidate.file_sha256
            || hashes
                .insert(file_name, candidate.file_sha256.clone())
                .is_some()
        {
            return invalid("candidate failed receipt or provenance verification");
        }
    }
    Ok(hashes)
}

fn copy_candidate(source: &Path, destination: &Path, expected_sha256: &str) -> ImportResult<()> {
    let mut source_file = open_private_path(source, false, false)?;
    if hash_open_file(&source_file)? != expected_sha256 {
        return invalid("candidate changed before it was copied");
    }
    source_file.rewind()?;
    let parent = destination
        .parent()
        .ok_or_else(|| ImportError::InvalidStage("destination has no parent".to_owned()))?;
    validate_destination_parent(parent)?;
    let mut destination_file = create_private_new(destination)?;
    std::io::copy(&mut source_file, &mut destination_file)?;
    destination_file.sync_all()?;
    drop(destination_file);
    if hash_private_file(destination)? != expected_sha256 {
        return invalid("candidate changed while it was copied");
    }
    sync_directory(parent)?;
    Ok(())
}

/// Durably flushes a directory after a namespace mutation within it.
#[cfg(unix)]
fn sync_directory(path: &Path) -> std::io::Result<()> {
    File::open(path)?.sync_all()
}

/// Durably flushes a directory after a namespace mutation within it.
///
/// `std::fs::File::open` cannot open a directory handle on Windows, so the
/// directory is opened explicitly with `FILE_FLAG_BACKUP_SEMANTICS`.
#[cfg(windows)]
fn sync_directory(path: &Path) -> std::io::Result<()> {
    use std::os::windows::fs::OpenOptionsExt;

    /// Lets `CreateFileW` open a directory handle.
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;

    let writable = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path);
    match writable {
        Ok(directory) => directory.sync_all(),
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            // FlushFileBuffers needs a writable handle, which some directories
            // refuse. Directory-metadata durability is handled by NTFS
            // journaling, and a read-only handle cannot FlushFileBuffers, so
            // verify the directory is reachable read-only and skip the flush.
            OpenOptions::new()
                .read(true)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
                .open(path)
                .map(|_| ())
        }
        Err(error) => Err(error),
    }
}

fn canonical_bytes<T: Serialize>(value: &T) -> ImportResult<Vec<u8>> {
    let mut bytes = serde_json::to_vec(value)
        .map_err(|error| ImportError::InvalidStage(format!("encode cutover record: {error}")))?;
    bytes.push(b'\n');
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return invalid("cutover record exceeds the fixed limit");
    }
    Ok(bytes)
}

fn read_canonical<T: for<'de> Deserialize<'de> + Serialize>(path: &Path) -> ImportResult<T> {
    let bytes = read_bounded(path)?;
    let value = serde_json::from_slice(&bytes)
        .map_err(|error| ImportError::InvalidStage(format!("decode cutover record: {error}")))?;
    if canonical_bytes(&value)? != bytes {
        return invalid("cutover record is not canonical JSON");
    }
    Ok(value)
}

fn read_bounded(path: &Path) -> ImportResult<Vec<u8>> {
    let file = open_private_path(path, false, false)?;
    let length = file.metadata()?.len();
    if length == 0 || length > MAX_RECORD_BYTES {
        return invalid("cutover record size is invalid");
    }
    let mut bytes = Vec::with_capacity(
        usize::try_from(length)
            .map_err(|_| ImportError::InvalidStage("cutover record size overflow".to_owned()))?,
    );
    file.take(MAX_RECORD_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != length || bytes.last() != Some(&b'\n') {
        return invalid("cutover record is truncated or noncanonical");
    }
    Ok(bytes)
}

fn publish_immutable_or_verify(path: &Path, bytes: &[u8]) -> ImportResult<()> {
    if path.exists() {
        if read_bounded(path)? != bytes {
            return invalid("immutable cutover record changed");
        }
        return Ok(());
    }
    publish_new(path, bytes)
}

fn publish_new(path: &Path, bytes: &[u8]) -> ImportResult<()> {
    let mut file = create_private_new(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    sync_private_directory(path.parent().expect("record path parent"))?;
    Ok(())
}

fn publish_replace(path: &Path, bytes: &[u8]) -> ImportResult<()> {
    let sequence = std::process::id();
    let temporary = path.with_extension(format!("json.{sequence}.tmp"));
    publish_new(&temporary, bytes)?;
    replace_private_file(&temporary, path)?;
    sync_private_directory(path.parent().expect("record path parent"))?;
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn hash_private_file(path: &Path) -> ImportResult<String> {
    let file = open_private_path(path, false, false)?;
    hash_open_file(&file)
}

fn kind_name(kind: StoreKind) -> &'static str {
    match kind {
        StoreKind::Global => "global",
        StoreKind::Project => "project",
    }
}

struct LegacyFence {
    path: PathBuf,
    lease: LegacyReadLease,
    identity: SourceIdentity,
    sha256: String,
}

fn acquire_legacy_fences(loaded: &LoadedStage) -> ImportResult<Vec<LegacyFence>> {
    let mut fences = Vec::with_capacity(loaded.databases.len());
    for (database, _) in &loaded.databases {
        fences.push(LegacyFence::acquire(database)?);
    }
    Ok(fences)
}

impl LegacyFence {
    fn acquire(database: &DatabaseEntry) -> ImportResult<Self> {
        let path = PathBuf::from(&database.source_path);
        let lease = acquire_legacy_read_lease(&path)?;
        let file = lease.try_clone_file()?;
        let metadata = file.metadata()?;
        validate_source_metadata(&database.source_identity, lease.identity(), &metadata)?;
        let sha256 = hash_open_file(&file)?;
        if sha256 != database.source_identity.sha256 {
            return invalid("legacy source digest changed after export");
        }
        if verify_legacy_source_identity(&path)? != lease.identity() {
            return invalid("legacy source identity changed while locking");
        }
        Ok(Self {
            path,
            lease,
            identity: database.source_identity.clone(),
            sha256,
        })
    }

    fn revalidate(&self) -> ImportResult<()> {
        let file = self.lease.try_clone_file()?;
        let metadata = file.metadata()?;
        validate_source_metadata(&self.identity, self.lease.identity(), &metadata)?;
        if verify_legacy_source_identity(&self.path)? != self.lease.identity()
            || hash_open_file(&file)? != self.sha256
        {
            return invalid("legacy source changed during activation");
        }
        Ok(())
    }
}

fn acquire_receipt_legacy_fences(
    sources: &[LegacySourceReceipt],
) -> ImportResult<Vec<LegacyFence>> {
    let mut fences = Vec::with_capacity(sources.len());
    for source in sources {
        let identity = SourceIdentity {
            device: source.device.clone(),
            inode: source.inode.clone(),
            size: source.size.clone(),
            mtime_seconds: source.mtime_seconds.clone(),
            mtime_nanos: source.mtime_nanos.clone(),
            sha256: source.sha256.clone(),
        };
        let path = PathBuf::from(&source.path);
        let lease = acquire_legacy_read_lease(&path)?;
        let file = lease.try_clone_file()?;
        validate_source_metadata(&identity, lease.identity(), &file.metadata()?)?;
        let sha256 = hash_open_file(&file)?;
        if sha256 != source.sha256 || verify_legacy_source_identity(&path)? != lease.identity() {
            return invalid("legacy source does not match the activation receipt");
        }
        fences.push(LegacyFence {
            path,
            lease,
            identity,
            sha256,
        });
    }
    Ok(fences)
}

fn validate_source_metadata(
    identity: &SourceIdentity,
    actual: ptrack_store::PrivatePathIdentity,
    metadata: &fs::Metadata,
) -> ImportResult<()> {
    let modified = metadata
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ImportError::InvalidStage("legacy source mtime is invalid".to_owned()))?;
    if identity.device != actual.device.to_string()
        || identity.inode != actual.inode.to_string()
        || identity.size != metadata.len().to_string()
        || identity.mtime_seconds != modified.as_secs().to_string()
        || identity.mtime_nanos != modified.subsec_nanos().to_string()
    {
        return invalid("legacy source identity changed after export");
    }
    Ok(())
}

fn hash_open_file(file: &File) -> ImportResult<String> {
    let mut file = file.try_clone()?;
    file.rewind()?;
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

fn revalidate_fences(fences: &[LegacyFence]) -> ImportResult<()> {
    for fence in fences {
        fence.revalidate()?;
    }
    Ok(())
}

fn validate_private_directory(path: &Path) -> ImportResult<()> {
    verify_private_path(path, true)?;
    Ok(())
}

#[cfg(unix)]
fn validate_destination_parent(path: &Path) -> ImportResult<()> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    let current = fs::symlink_metadata(path)?;
    if current.file_type().is_symlink() || !current.is_dir() {
        return invalid("destination parent must be a real directory");
    }
    let opened = OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits().cast_signed())
        .open(path)?
        .metadata()?;
    if current.dev() != opened.dev() || current.ino() != opened.ino() {
        return invalid("destination parent changed while it was opened");
    }
    Ok(())
}

#[cfg(windows)]
fn validate_destination_parent(path: &Path) -> ImportResult<()> {
    verify_private_path(path, true)?;
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn validate_destination_parent(_: &Path) -> ImportResult<()> {
    invalid("destination parent verification is unsupported")
}

#[cfg(unix)]
fn create_private_new(path: &Path) -> ImportResult<File> {
    use std::os::unix::fs::OpenOptionsExt;

    Ok(OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?)
}

#[cfg(windows)]
fn create_private_new(path: &Path) -> ImportResult<File> {
    let file = OpenOptions::new().write(true).create_new(true).open(path)?;
    protect_private_file(path)?;
    Ok(file)
}

#[cfg(not(any(unix, windows)))]
fn create_private_new(_: &Path) -> ImportResult<File> {
    invalid("private cutover file creation is unsupported on this platform")
}
