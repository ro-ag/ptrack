use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use ptrack_core::{MemoryKind, NativeRecord, RecordKind, decode_record};
use ptrack_store::{
    Collection, ImportCollection, ImportRecord, JsonStageImportData, MAX_IMPORT_BYTES,
    MAX_IMPORT_KEY_BYTES, MAX_IMPORT_PAYLOAD_BYTES, NATIVE_CODEC, NATIVE_PAYLOAD_SCHEMA,
    OwnedRecordKey, RecordEnvelope, StoreKind,
};

use crate::error::{ImportError, ImportResult, invalid};
use crate::manifest::{
    DatabaseEntry, DatabaseKind, clean_absolute, decimal_i64, decimal_u64, decode_manifest,
};
use crate::sha256::{Sha256, hex};
use crate::wire::{
    GLOBAL_COLLECTIONS, Line, PROJECT_COLLECTIONS, decode_digest, validate_collection_set,
};

const MAX_MANIFEST_BYTES: u64 = 8 * 1024 * 1024;
const MAX_DATABASE_JSON_BYTES: u64 = 768 * 1024 * 1024;
const MAX_JSON_LINE_BYTES: u64 = 513 * 1024 * 1024;
const MAX_RECORDS: u64 = 1_000_000;
const MAX_BUCKETS: u64 = 13;
const RECORD_ENVELOPE_OVERHEAD: usize = 20;
const QUARANTINE_KEY_OVERHEAD: usize = 6;
const QUARANTINE_VALUE_OVERHEAD: usize = 41;

/// Verified facts about one immutable JSON database stage.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StageReport {
    /// SHA-256 of the exact canonical manifest bytes.
    pub manifest_sha256: [u8; 32],
    /// Number of databases described and fully validated.
    pub database_count: u64,
    /// Total normal plus quarantined source records.
    pub record_count: u64,
    /// Total inert quarantined records.
    pub quarantine_count: u64,
}

/// Validates a complete immutable stage without inspecting or creating a destination.
///
/// # Errors
///
/// Returns an error for unsafe paths, unbounded or changed inputs, digest/count/order
/// mismatches, unknown schema fields, invalid typed records, or invalid cross-record state.
pub fn validate_stage(manifest_path: &Path) -> ImportResult<StageReport> {
    load_stage(manifest_path).map(|stage| stage.report)
}

pub(crate) struct LoadedStage {
    pub report: StageReport,
    pub databases: Vec<(DatabaseEntry, JsonStageImportData)>,
}

pub(crate) fn load_stage(manifest_path: &Path) -> ImportResult<LoadedStage> {
    if !manifest_path.is_absolute() {
        return invalid("manifest path must be absolute");
    }
    if manifest_path
        .file_name()
        .is_none_or(|name| name != "manifest.json")
    {
        return invalid("staging manifest must be named manifest.json");
    }
    let manifest_bytes = read_stable_private_file(manifest_path, MAX_MANIFEST_BYTES)?;
    let mut manifest_hash = Sha256::new();
    manifest_hash.update(&manifest_bytes);
    let manifest_sha256 = manifest_hash.finish();
    let manifest = decode_manifest(&manifest_bytes)?;
    if manifest.databases.first().map(|database| database.kind) != Some(DatabaseKind::Global) {
        return invalid("migration batch must begin with exactly one global database");
    }
    let root = manifest_path
        .parent()
        .ok_or_else(|| ImportError::InvalidStage("manifest has no staging directory".to_owned()))?;
    validate_private_directory(root)?;
    validate_private_directory(&root.join("databases"))?;

    let mut total_records = 0_u64;
    let mut total_quarantine = 0_u64;
    let mut total_retained_bytes = 0_u64;
    let mut databases = Vec::with_capacity(manifest.databases.len());
    let mut registered_projects = None;
    for database in &manifest.databases {
        let mut parsed = validate_database(root, database)?;
        total_records = checked_count(total_records, parsed.record_count, "record count")?;
        total_quarantine = checked_count(
            total_quarantine,
            parsed.quarantine_count,
            "quarantine count",
        )?;
        total_retained_bytes = checked_retained_bytes(
            total_retained_bytes,
            usize::try_from(parsed.retained_bytes).map_err(|_| {
                ImportError::InvalidStage("retained byte count does not fit usize".to_owned())
            })?,
        )?;
        parsed.import.batch_manifest_sha256 = manifest_sha256;
        if database.kind == DatabaseKind::Global {
            registered_projects = Some(parsed.registered_projects);
        }
        databases.push((database.clone(), parsed.import));
    }
    if total_quarantine != decimal_u64(&manifest.quarantine_count, "quarantine_count")? {
        return invalid("manifest quarantine_count does not match JSONL records");
    }
    let registered_source_paths = manifest
        .registry
        .iter()
        .map(|entry| entry.source_path.clone())
        .collect::<BTreeSet<_>>();
    if registered_projects.as_ref() != Some(&registered_source_paths) {
        return invalid("registry metadata does not match global project records");
    }
    Ok(LoadedStage {
        report: StageReport {
            manifest_sha256,
            database_count: manifest.databases.len() as u64,
            record_count: total_records,
            quarantine_count: total_quarantine,
        },
        databases,
    })
}

struct ParsedDatabase {
    record_count: u64,
    quarantine_count: u64,
    retained_bytes: u64,
    registered_projects: BTreeSet<String>,
    import: JsonStageImportData,
}

#[allow(clippy::too_many_lines)]
fn validate_database(root: &Path, database: &DatabaseEntry) -> ImportResult<ParsedDatabase> {
    let path = database.data.resolved(root);
    let expected_bytes = decimal_u64(&database.data.bytes, "data.bytes")?;
    if expected_bytes > MAX_DATABASE_JSON_BYTES {
        return invalid("database JSONL exceeds the fixed byte limit");
    }
    let file = open_stable_private_file(&path, expected_bytes)?;
    if file.metadata()?.len() != expected_bytes {
        return invalid("database JSONL byte length does not match its manifest");
    }
    let mut reader = BufReader::new(file);
    let mut hash = Sha256::new();
    let mut line = Vec::new();
    let mut line_number = 0_u64;
    let mut header_seen = false;
    let mut header_quarantine_count = None;
    let mut current_bucket: Option<BucketState> = None;
    let mut bucket_names = BTreeSet::new();
    let mut bucket_sequences = BTreeMap::new();
    let mut bucket_count = 0_u64;
    let mut record_count = 0_u64;
    let mut quarantine_count = 0_u64;
    let mut retained_bytes = 0_u64;
    let mut cross_records = CrossRecords::default();
    let mut import_collections = Vec::new();
    let mut import_records = Vec::new();
    let mut quarantined = Vec::new();
    let mut json_bytes = 0_u64;
    loop {
        line.clear();
        let bytes = reader
            .by_ref()
            .take(MAX_JSON_LINE_BYTES + 1)
            .read_until(b'\n', &mut line)?;
        if bytes == 0 {
            break;
        }
        json_bytes = json_bytes.checked_add(bytes as u64).ok_or_else(|| {
            ImportError::InvalidStage("database JSONL byte count overflow".to_owned())
        })?;
        line_number += 1;
        if u64::try_from(line.len()).expect("line length fits u64") > MAX_JSON_LINE_BYTES {
            return invalid(format!(
                "JSONL line {line_number} exceeds the fixed byte limit"
            ));
        }
        if !line.ends_with(b"\n") || line[..line.len() - 1].contains(&b'\n') {
            return invalid(format!("JSONL line {line_number} is not LF terminated"));
        }
        if !compact_json(&line[..line.len() - 1]) {
            return invalid(format!("JSONL line {line_number} is not compact JSON"));
        }
        hash.update(&line);
        let parsed = Line::decode(&line[..line.len() - 1]).map_err(|error| {
            ImportError::InvalidStage(format!("decode JSONL line {line_number}: {error}"))
        })?;
        match parsed {
            Line::Header(header) => {
                if header_seen || line_number != 1 {
                    return invalid("database header must be the first and only header line");
                }
                header_seen = true;
                let kind = match database.kind {
                    DatabaseKind::Global => "global",
                    DatabaseKind::Project => "project",
                };
                if header.schema != "1"
                    || header.database_id != database.id
                    || header.kind != kind
                    || header.source_format != database.source_format
                    || decimal_u64(&header.bucket_count, "header.bucket_count")?
                        != decimal_u64(&database.data.bucket_count, "data.bucket_count")?
                    || decimal_u64(&header.record_count, "header.record_count")?
                        != decimal_u64(&database.data.record_count, "data.record_count")?
                {
                    return invalid("database header does not match its manifest entry");
                }
                header_quarantine_count = Some(decimal_u64(
                    &header.quarantine_count,
                    "header.quarantine_count",
                )?);
            }
            Line::Bucket(bucket) => {
                if !header_seen {
                    return invalid("bucket appears before database header");
                }
                finish_bucket(
                    &mut current_bucket,
                    &mut import_collections,
                    &mut import_records,
                )?;
                let bucket_index = usize::try_from(bucket_count).map_err(|_| {
                    ImportError::InvalidStage("bucket count does not fit usize".to_owned())
                })?;
                let expected = collection_order(database.kind)
                    .get(bucket_index)
                    .ok_or_else(|| ImportError::InvalidStage("too many bucket lines".to_owned()))?;
                if bucket.name != *expected {
                    return invalid("bucket lines are not in the fixed legacy collection order");
                }
                if !bucket_names.insert(bucket.name.clone()) {
                    return invalid("duplicate bucket line");
                }
                bucket_count = checked_count(bucket_count, 1, "bucket count")?;
                if bucket_count > MAX_BUCKETS {
                    return invalid("bucket count exceeds the fixed limit");
                }
                let declared = decimal_u64(&bucket.record_count, "bucket.record_count")?;
                if !bucket.present && declared != 0 {
                    return invalid("an absent bucket cannot contain records");
                }
                let sequenced = is_sequenced(&bucket.name);
                let sequence = match (sequenced, bucket.present, bucket.sequence.as_deref()) {
                    (true, true, Some(value)) => Some(decimal_u64(value, "bucket.sequence")?),
                    (true, false, None) | (false, _, None) => None,
                    _ => return invalid("bucket sequence presence is invalid"),
                };
                validate_bucket_presence(database, &bucket.name, bucket.present)?;
                bucket_sequences.insert(bucket.name.clone(), sequence);
                current_bucket = Some(BucketState {
                    name: bucket.name,
                    present: bucket.present,
                    sequence,
                    declared,
                    seen: 0,
                    previous_key: None,
                    maximum_numeric_key: 0,
                });
            }
            Line::Record(record) => {
                let bucket = current_bucket.as_mut().ok_or_else(|| {
                    ImportError::InvalidStage("record appears before a bucket".to_owned())
                })?;
                if record.bucket != bucket.name {
                    return invalid("record names the wrong bucket");
                }
                if !bucket.present {
                    return invalid("record appears in an absent bucket");
                }
                let converted = record.convert()?;
                validate_retained_key(&converted.key)?;
                bucket.add_key(&converted.key)?;
                bucket.seen = checked_count(bucket.seen, 1, "bucket record count")?;
                record_count = checked_count(record_count, 1, "record count")?;
                if converted.payload.len() as u64 > MAX_IMPORT_PAYLOAD_BYTES {
                    return invalid("record payload exceeds the store import limit");
                }
                retained_bytes = checked_retained_bytes(
                    retained_bytes,
                    converted
                        .key
                        .len()
                        .checked_add(converted.payload.len())
                        .and_then(|value| value.checked_add(RECORD_ENVELOPE_OVERHEAD))
                        .ok_or_else(|| {
                            ImportError::InvalidStage("retained byte count overflow".to_owned())
                        })?,
                )?;
                validate_raw_record(
                    &bucket.name,
                    &converted.key,
                    &converted.payload,
                    converted.raw,
                )?;
                if !converted.raw {
                    let kind = record_kind(&bucket.name).ok_or_else(|| {
                        ImportError::InvalidStage("native record is in a raw bucket".to_owned())
                    })?;
                    let native = decode_record(kind, &converted.payload).map_err(|error| {
                        ImportError::InvalidStage(format!(
                            "canonical record did not re-decode: {error}"
                        ))
                    })?;
                    validate_special_key(&bucket.name, &converted.key, &native)?;
                    cross_records.add(&native)?;
                }
                import_records.push(ImportRecord {
                    key: owned_key(&bucket.name, converted.key)?,
                    envelope: RecordEnvelope::new(
                        if converted.raw {
                            ptrack_store::LEGACY_CODEC_RAW
                        } else {
                            NATIVE_CODEC
                        },
                        if converted.raw {
                            0
                        } else {
                            NATIVE_PAYLOAD_SCHEMA
                        },
                        converted.payload,
                    ),
                });
            }
            Line::Quarantine(quarantine_line) => {
                let bucket = current_bucket.as_mut().ok_or_else(|| {
                    ImportError::InvalidStage("quarantine appears before a bucket".to_owned())
                })?;
                if quarantine_line.bucket != bucket.name {
                    return invalid("quarantine names the wrong bucket");
                }
                if !bucket.present {
                    return invalid("quarantine appears in an absent bucket");
                }
                let (key, retained) = quarantine_line.convert()?;
                validate_retained_key(&key)?;
                bucket.add_key(&key)?;
                bucket.seen = checked_count(bucket.seen, 1, "bucket record count")?;
                record_count = checked_count(record_count, 1, "record count")?;
                quarantine_count = checked_count(quarantine_count, 1, "quarantine count")?;
                retained_bytes = checked_retained_bytes(
                    retained_bytes,
                    key.len()
                        .checked_add(retained.source_bucket.len())
                        .and_then(|value| value.checked_add(retained.legacy_gob.len()))
                        .and_then(|value| {
                            value.checked_add(QUARANTINE_KEY_OVERHEAD + QUARANTINE_VALUE_OVERHEAD)
                        })
                        .ok_or_else(|| {
                            ImportError::InvalidStage("retained byte count overflow".to_owned())
                        })?,
                )?;
                quarantined.push(retained);
            }
        }
    }
    finish_bucket(
        &mut current_bucket,
        &mut import_collections,
        &mut import_records,
    )?;
    if !header_seen {
        return invalid("database JSONL is missing its header");
    }
    validate_collection_set(&bucket_names, database.kind == DatabaseKind::Project)?;
    validate_required_collections(&bucket_names, database)?;
    let expected_records = decimal_u64(&database.data.record_count, "data.record_count")?;
    let expected_buckets = decimal_u64(&database.data.bucket_count, "data.bucket_count")?;
    if record_count != expected_records
        || bucket_count != expected_buckets
        || json_bytes != expected_bytes
        || header_quarantine_count != Some(quarantine_count)
    {
        return invalid("database JSONL count does not match its manifest");
    }
    if hex(hash.finish()) != database.data.sha256 {
        return invalid("database JSONL SHA-256 does not match its manifest");
    }
    cross_records.validate(database, &bucket_sequences)?;
    synthesize_absent_collections(database.kind, &mut import_collections);
    import_collections.sort_by_key(|value| value.collection);
    let database_json_sha256 = decode_digest(&database.data.sha256, "data.sha256")?;
    Ok(ParsedDatabase {
        record_count,
        quarantine_count,
        retained_bytes,
        registered_projects: cross_records.project_refs,
        import: JsonStageImportData {
            kind: store_kind(database.kind),
            source_format: decimal_u64(&database.source_format, "source_format")?,
            batch_manifest_sha256: [0; 32],
            database_json_sha256,
            collections: import_collections,
            quarantine: quarantined,
        },
    })
}

struct BucketState {
    name: String,
    present: bool,
    sequence: Option<u64>,
    declared: u64,
    seen: u64,
    previous_key: Option<Vec<u8>>,
    maximum_numeric_key: u64,
}

impl BucketState {
    fn add_key(&mut self, key: &[u8]) -> ImportResult<()> {
        if self
            .previous_key
            .as_deref()
            .is_some_and(|previous| previous >= key)
        {
            return invalid("records are not in strict raw key order");
        }
        self.previous_key = Some(key.to_vec());
        if is_numeric_keyed(&self.name) {
            self.maximum_numeric_key =
                self.maximum_numeric_key
                    .max(u64::from_be_bytes(key.try_into().map_err(|_| {
                        ImportError::InvalidStage("numeric key is not eight bytes".to_owned())
                    })?));
        }
        Ok(())
    }
}

fn finish_bucket(
    bucket: &mut Option<BucketState>,
    collections: &mut Vec<ImportCollection>,
    records: &mut Vec<ImportRecord>,
) -> ImportResult<()> {
    if let Some(value) = bucket.as_ref() {
        if value.seen != value.declared {
            return invalid("bucket record_count does not match its lines");
        }
        if value.present
            && value
                .sequence
                .is_some_and(|sequence| sequence < value.maximum_numeric_key)
        {
            return invalid("bucket sequence is below its maximum numeric key");
        }
    }
    if let Some(value) = bucket.take()
        && value.present
    {
        collections.push(ImportCollection {
            collection: Collection::from_legacy_name(value.name.as_bytes())
                .expect("known bucket was validated"),
            records: std::mem::take(records),
            sequence: value.sequence,
        });
    } else if !records.is_empty() {
        return invalid("an absent bucket retained records");
    }
    Ok(())
}

fn validate_required_collections(
    names: &BTreeSet<String>,
    database: &DatabaseEntry,
) -> ImportResult<()> {
    let source = decimal_u64(&database.source_format, "source_format")?;
    match database.kind {
        DatabaseKind::Global if source != 0 => return invalid("global source_format must be zero"),
        DatabaseKind::Project if source > 5 => {
            return invalid("project source_format exceeds the supported legacy format");
        }
        DatabaseKind::Global | DatabaseKind::Project => {}
    }
    let expected = match database.kind {
        DatabaseKind::Global => GLOBAL_COLLECTIONS.as_slice(),
        DatabaseKind::Project => PROJECT_COLLECTIONS.as_slice(),
    };
    for name in expected {
        if !names.contains(*name) {
            return invalid(format!("required collection {name:?} is missing"));
        }
    }
    Ok(())
}

fn synthesize_absent_collections(kind: DatabaseKind, collections: &mut Vec<ImportCollection>) {
    for collection in Collection::for_store(store_kind(kind)) {
        if !collections
            .iter()
            .any(|value| value.collection == collection)
        {
            collections.push(ImportCollection {
                collection,
                records: Vec::new(),
                sequence: collection.is_sequenced().then_some(0),
            });
        }
    }
}

const fn store_kind(kind: DatabaseKind) -> StoreKind {
    match kind {
        DatabaseKind::Global => StoreKind::Global,
        DatabaseKind::Project => StoreKind::Project,
    }
}

fn owned_key(bucket: &str, key: Vec<u8>) -> ImportResult<OwnedRecordKey> {
    Ok(if bucket == "meta" {
        OwnedRecordKey::Singleton
    } else if is_numeric_keyed(bucket) {
        OwnedRecordKey::Id(u64::from_be_bytes(key.try_into().map_err(|_| {
            ImportError::InvalidStage("numeric key is not eight bytes".to_owned())
        })?))
    } else {
        OwnedRecordKey::Bytes(key)
    })
}

#[derive(Default)]
struct CrossRecords {
    meta: Option<(u64, u64)>,
    plans: BTreeSet<u64>,
    plan_milestones: Vec<u64>,
    tasks: BTreeSet<u64>,
    task_plans: Vec<u64>,
    milestones: BTreeSet<u64>,
    issue_tasks: Vec<u64>,
    notes: BTreeMap<u64, MemoryKind>,
    writebacks: Vec<(u64, MemoryKind, u64)>,
    project_refs: BTreeSet<String>,
}

impl CrossRecords {
    fn add(&mut self, record: &NativeRecord) -> ImportResult<()> {
        match record {
            NativeRecord::Meta(value) => {
                if self
                    .meta
                    .replace((value.format_version, value.active_plan))
                    .is_some()
                {
                    return invalid("project contains more than one meta record");
                }
            }
            NativeRecord::Plan(value) => {
                self.plans.insert(value.id);
                if value.milestone_id != 0 {
                    self.plan_milestones.push(value.milestone_id);
                }
            }
            NativeRecord::Task(value) => {
                self.tasks.insert(value.id);
                self.task_plans.push(value.plan_id);
            }
            NativeRecord::Milestone(value) => {
                self.milestones.insert(value.id);
            }
            NativeRecord::Issue(value) if value.task_id != 0 => {
                self.issue_tasks.push(value.task_id);
            }
            NativeRecord::Note(value) => {
                self.notes.insert(value.id, value.kind);
            }
            NativeRecord::MemoryWriteback(value) => {
                self.writebacks
                    .push((value.sequence, value.kind, value.note_id));
            }
            NativeRecord::ProjectRef(value) => {
                self.project_refs.insert(value.path.clone());
            }
            NativeRecord::Commit(_)
            | NativeRecord::Capability(_)
            | NativeRecord::CapabilityAudit(_)
            | NativeRecord::Issue(_) => {}
        }
        Ok(())
    }

    fn validate(
        &self,
        database: &DatabaseEntry,
        sequences: &BTreeMap<String, Option<u64>>,
    ) -> ImportResult<()> {
        if database.kind == DatabaseKind::Global {
            return Ok(());
        }
        let (format, active_plan) = self
            .meta
            .ok_or_else(|| ImportError::InvalidStage("project meta is missing".to_owned()))?;
        if format != decimal_u64(&database.source_format, "source_format")?
            || (active_plan != 0 && !self.plans.contains(&active_plan))
        {
            return invalid("meta format or active plan is invalid");
        }
        if self
            .plan_milestones
            .iter()
            .any(|id| !self.milestones.contains(id))
        {
            return invalid("plan milestone is missing");
        }
        if self.task_plans.iter().any(|id| !self.plans.contains(id)) {
            return invalid("task plan is missing");
        }
        if self.issue_tasks.iter().any(|id| !self.tasks.contains(id)) {
            return invalid("issue task is missing");
        }
        let high_water = sequences
            .get("memory_writebacks")
            .copied()
            .flatten()
            .unwrap_or(0);
        let mut receipt_sequences = BTreeSet::new();
        let mut receipt_notes = BTreeSet::new();
        for &(sequence, kind, note_id) in &self.writebacks {
            if sequence > high_water {
                return invalid("writeback sequence exceeds its bucket high-water mark");
            }
            if !receipt_sequences.insert(sequence) {
                return invalid("writeback sequence is duplicated");
            }
            if kind != MemoryKind::Summary
                && (self.notes.get(&note_id) != Some(&kind) || !receipt_notes.insert(note_id))
            {
                return invalid("writeback note relationship is invalid");
            }
        }
        Ok(())
    }
}

fn validate_raw_record(bucket: &str, key: &[u8], value: &[u8], raw: bool) -> ImportResult<()> {
    if matches!(bucket, "config" | "backups") != raw {
        return invalid("raw/native model does not match bucket");
    }
    if bucket == "config" && key.is_empty() {
        return invalid("global config key is empty");
    }
    if bucket == "backups" {
        let key = std::str::from_utf8(key)
            .map_err(|_| ImportError::InvalidStage("backup key is not UTF-8".to_owned()))?;
        let timestamp = decimal_i64(key, "backup key")?;
        let value = std::str::from_utf8(value)
            .map_err(|_| ImportError::InvalidStage("backup value is not UTF-8".to_owned()))?;
        let mut paths = value.split('\t');
        if timestamp < 0
            || paths.next().is_none_or(str::is_empty)
            || paths.next().is_none_or(str::is_empty)
            || paths.next().is_some()
        {
            return invalid("global backup raw record is invalid");
        }
    }
    Ok(())
}

fn validate_special_key(bucket: &str, key: &[u8], record: &NativeRecord) -> ImportResult<()> {
    if let NativeRecord::ProjectRef(value) = record
        && (key != value.path.as_bytes() || !clean_absolute(Path::new(&value.path)))
    {
        return invalid("project reference key/path is not canonical absolute");
    }
    if bucket == "memory_writebacks"
        && (key.len() > 128
            || !key.iter().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':')
            }))
    {
        return invalid("memory writeback key is invalid");
    }
    Ok(())
}

fn record_kind(bucket: &str) -> Option<RecordKind> {
    Some(match bucket {
        "meta" => RecordKind::Meta,
        "plans" => RecordKind::Plan,
        "tasks" => RecordKind::Task,
        "notes" => RecordKind::Note,
        "milestones" => RecordKind::Milestone,
        "issues" => RecordKind::Issue,
        "commits" => RecordKind::Commit,
        "capabilities" => RecordKind::Capability,
        "capability_audits" => RecordKind::CapabilityAudit,
        "memory_writebacks" => RecordKind::MemoryWriteback,
        "projects" => RecordKind::ProjectRef,
        _ => return None,
    })
}
fn is_sequenced(name: &str) -> bool {
    matches!(
        name,
        "plans"
            | "tasks"
            | "notes"
            | "milestones"
            | "issues"
            | "commits"
            | "capabilities"
            | "capability_audits"
            | "memory_writebacks"
    )
}
fn is_numeric_keyed(name: &str) -> bool {
    matches!(
        name,
        "plans"
            | "tasks"
            | "notes"
            | "milestones"
            | "issues"
            | "commits"
            | "capabilities"
            | "capability_audits"
    )
}
fn collection_order(kind: DatabaseKind) -> &'static [&'static str] {
    match kind {
        DatabaseKind::Global => GLOBAL_COLLECTIONS.as_slice(),
        DatabaseKind::Project => PROJECT_COLLECTIONS.as_slice(),
    }
}
fn introduced_in(name: &str) -> u64 {
    match name {
        "milestones" | "issues" => 2,
        "commits" => 3,
        "capabilities" | "capability_audits" => 4,
        "memory_writebacks" => 5,
        _ => 0,
    }
}
pub(crate) fn validate_bucket_presence(
    database: &DatabaseEntry,
    name: &str,
    present: bool,
) -> ImportResult<()> {
    let source = decimal_u64(&database.source_format, "source_format")?;
    if !present && (database.kind == DatabaseKind::Global || introduced_in(name) <= source) {
        return invalid(format!(
            "required bucket {name:?} is absent for source format {source}"
        ));
    }
    Ok(())
}
fn checked_count(current: u64, added: u64, name: &str) -> ImportResult<u64> {
    let value = current
        .checked_add(added)
        .ok_or_else(|| ImportError::InvalidStage(format!("{name} overflow")))?;
    if value > MAX_RECORDS {
        return invalid(format!("{name} exceeds {MAX_RECORDS}"));
    }
    Ok(value)
}
fn checked_retained_bytes(current: u64, added: usize) -> ImportResult<u64> {
    let value = current
        .checked_add(added as u64)
        .ok_or_else(|| ImportError::InvalidStage("native byte count overflow".to_owned()))?;
    if value > MAX_IMPORT_BYTES {
        return invalid("retained import bytes exceed the store limit");
    }
    Ok(value)
}

fn validate_retained_key(key: &[u8]) -> ImportResult<()> {
    if key.len() as u64 > MAX_IMPORT_KEY_BYTES {
        return invalid("record key exceeds the store import limit");
    }
    Ok(())
}

fn compact_json(bytes: &[u8]) -> bool {
    let mut quoted = false;
    let mut escaped = false;
    for &byte in bytes {
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
        } else if byte == b'"' {
            quoted = true;
        } else if byte.is_ascii_whitespace() {
            return false;
        }
    }
    !quoted && !escaped
}

fn read_stable_private_file(path: &Path, limit: u64) -> ImportResult<Vec<u8>> {
    let file = open_stable_private_file(path, limit)?;
    let mut data = Vec::new();
    file.take(limit + 1).read_to_end(&mut data)?;
    if data.len() as u64 > limit {
        return invalid("file exceeds fixed byte limit");
    }
    Ok(data)
}

fn open_stable_private_file(path: &Path, expected_or_limit: u64) -> ImportResult<File> {
    let before = fs::symlink_metadata(path)?;
    if before.file_type().is_symlink() || !before.is_file() {
        return invalid("stage artifact must be a regular non-symlink file");
    }
    validate_private_file(path, &before)?;
    if before.len() > expected_or_limit {
        return invalid("stage artifact size exceeds expected or fixed limit");
    }
    let identity = ptrack_store::verify_private_path(path, false)?;
    let file = ptrack_store::open_private_path(path, false, false)?;
    let opened = file.metadata()?;
    if ptrack_store::verify_private_path(path, false)? != identity || opened.len() != before.len() {
        return invalid("stage artifact changed while opening");
    }
    Ok(file)
}

#[cfg(unix)]
fn validate_private_file(_: &Path, info: &fs::Metadata) -> ImportResult<()> {
    use std::os::unix::fs::PermissionsExt;
    if info.permissions().mode() & 0o777 != 0o600 {
        return invalid("stage artifact permissions must be 0600");
    }
    Ok(())
}
#[cfg(windows)]
fn validate_private_file(path: &Path, _: &fs::Metadata) -> ImportResult<()> {
    ptrack_store::verify_private_path(path, false)?;
    Ok(())
}
#[cfg(not(any(unix, windows)))]
fn validate_private_file(_: &Path, _: &fs::Metadata) -> ImportResult<()> {
    invalid("stage file privacy is unsupported on this platform")
}
#[cfg(unix)]
fn validate_private_directory(path: &Path) -> ImportResult<()> {
    use std::os::unix::fs::PermissionsExt;
    let info = fs::symlink_metadata(path)?;
    if info.file_type().is_symlink() || !info.is_dir() || info.permissions().mode() & 0o777 != 0o700
    {
        return invalid("staging directories must be non-symlink directories with mode 0700");
    }
    Ok(())
}
#[cfg(windows)]
fn validate_private_directory(path: &Path) -> ImportResult<()> {
    ptrack_store::verify_private_path(path, true)?;
    Ok(())
}
#[cfg(not(any(unix, windows)))]
fn validate_private_directory(_: &Path) -> ImportResult<()> {
    invalid("staging directory privacy is unsupported on this platform")
}
