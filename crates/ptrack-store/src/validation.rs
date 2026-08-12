use std::path::{Component, Path, PathBuf};

use ptrack_core::{NativeRecord, RecordKind, decode_record, encode_record};

use crate::{
    Collection, LEGACY_CODEC_RAW, NATIVE_CODEC, NATIVE_PAYLOAD_SCHEMA, OwnedRecordKey,
    RecordEnvelope,
};

pub(crate) fn record(
    collection: Collection,
    key: &OwnedRecordKey,
    envelope: &RecordEnvelope,
) -> Result<(), String> {
    if envelope.codec() != collection.import_codec()
        || envelope.payload_schema() != collection.import_payload_schema()
    {
        return Err(format!(
            "collection {} requires codec {} schema {}, found codec {} schema {}",
            collection.name(),
            collection.import_codec(),
            collection.import_payload_schema(),
            envelope.codec(),
            envelope.payload_schema()
        ));
    }
    match collection {
        Collection::GlobalConfig => validate_config(key, envelope),
        Collection::GlobalBackups => validate_backup(key, envelope),
        _ => validate_native(collection, key, envelope),
    }
}

fn validate_native(
    collection: Collection,
    key: &OwnedRecordKey,
    envelope: &RecordEnvelope,
) -> Result<(), String> {
    if envelope.codec() != NATIVE_CODEC || envelope.payload_schema() != NATIVE_PAYLOAD_SCHEMA {
        return Err("native record has the wrong codec or payload schema".to_owned());
    }
    let kind = record_kind(collection)
        .ok_or_else(|| format!("collection {} has no native record kind", collection.name()))?;
    let decoded = decode_record(kind, envelope.payload())
        .map_err(|error| format!("invalid native {} record: {error}", collection.name()))?;
    let canonical = encode_record(&decoded).map_err(|error| {
        format!(
            "cannot re-encode native {} record: {error}",
            collection.name()
        )
    })?;
    if canonical != envelope.payload() {
        return Err(format!(
            "native {} payload is not canonical",
            collection.name()
        ));
    }
    validate_identity(collection, key, &decoded)
}

fn record_kind(collection: Collection) -> Option<RecordKind> {
    match collection {
        Collection::ProjectMeta => Some(RecordKind::Meta),
        Collection::Plans => Some(RecordKind::Plan),
        Collection::Tasks => Some(RecordKind::Task),
        Collection::Notes => Some(RecordKind::Note),
        Collection::Milestones => Some(RecordKind::Milestone),
        Collection::Issues => Some(RecordKind::Issue),
        Collection::Commits => Some(RecordKind::Commit),
        Collection::Capabilities => Some(RecordKind::Capability),
        Collection::CapabilityAudits => Some(RecordKind::CapabilityAudit),
        Collection::MemoryWritebacks => Some(RecordKind::MemoryWriteback),
        Collection::GlobalProjects => Some(RecordKind::ProjectRef),
        Collection::GlobalConfig | Collection::GlobalBackups => None,
    }
}

fn validate_identity(
    collection: Collection,
    key: &OwnedRecordKey,
    record: &NativeRecord,
) -> Result<(), String> {
    match (key, record) {
        (OwnedRecordKey::Singleton, NativeRecord::Meta(_)) => Ok(()),
        (OwnedRecordKey::Id(key), NativeRecord::Plan(value)) if *key == value.id => Ok(()),
        (OwnedRecordKey::Id(key), NativeRecord::Task(value)) if *key == value.id => Ok(()),
        (OwnedRecordKey::Id(key), NativeRecord::Note(value)) if *key == value.id => Ok(()),
        (OwnedRecordKey::Id(key), NativeRecord::Milestone(value)) if *key == value.id => Ok(()),
        (OwnedRecordKey::Id(key), NativeRecord::Issue(value)) if *key == value.id => Ok(()),
        (OwnedRecordKey::Id(key), NativeRecord::Commit(value)) if *key == value.id => Ok(()),
        (OwnedRecordKey::Id(key), NativeRecord::Capability(value)) if *key == value.id => Ok(()),
        (OwnedRecordKey::Id(key), NativeRecord::CapabilityAudit(value)) if *key == value.id => {
            Ok(())
        }
        (OwnedRecordKey::Bytes(key), NativeRecord::MemoryWriteback(_)) => {
            validate_memory_request_id(key)
        }
        (OwnedRecordKey::Bytes(key), NativeRecord::ProjectRef(value)) => {
            let key = std::str::from_utf8(key)
                .map_err(|_| "project registry key is not valid UTF-8".to_owned())?;
            if key != value.path {
                return Err("project registry key does not equal its payload path".to_owned());
            }
            if !is_clean_absolute(Path::new(&value.path)) {
                return Err("project registry path is not absolute and lexically clean".to_owned());
            }
            Ok(())
        }
        _ => Err(format!(
            "collection {} key does not identify its native payload",
            collection.name()
        )),
    }
}

fn validate_memory_request_id(key: &[u8]) -> Result<(), String> {
    if key.is_empty()
        || key.len() > 128
        || std::str::from_utf8(key).is_err()
        || !key.iter().all(|value| {
            value.is_ascii_alphanumeric() || matches!(value, b'-' | b'_' | b'.' | b':')
        })
    {
        Err("memory write-back key is not a canonical request ID".to_owned())
    } else {
        Ok(())
    }
}

fn validate_config(key: &OwnedRecordKey, envelope: &RecordEnvelope) -> Result<(), String> {
    if envelope.codec() != LEGACY_CODEC_RAW || envelope.payload_schema() != 0 {
        return Err("global config is not raw schema zero".to_owned());
    }
    let OwnedRecordKey::Bytes(key) = key else {
        return Err("global config key is not bytes".to_owned());
    };
    if key.is_empty() {
        return Err("global config key must be nonempty".to_owned());
    }
    Ok(())
}

fn validate_backup(key: &OwnedRecordKey, envelope: &RecordEnvelope) -> Result<(), String> {
    if envelope.codec() != LEGACY_CODEC_RAW || envelope.payload_schema() != 0 {
        return Err("global backup is not raw schema zero".to_owned());
    }
    let OwnedRecordKey::Bytes(key) = key else {
        return Err("global backup key is not bytes".to_owned());
    };
    let key =
        std::str::from_utf8(key).map_err(|_| "global backup key is not valid UTF-8".to_owned())?;
    let timestamp: i64 = key
        .parse()
        .map_err(|_| "global backup key is not a timestamp".to_owned())?;
    if timestamp < 0 || timestamp.to_string() != key {
        return Err("global backup key is not a canonical nonnegative timestamp".to_owned());
    }
    let value = std::str::from_utf8(envelope.payload())
        .map_err(|_| "global backup value is not valid UTF-8".to_owned())?;
    let Some((project, backup)) = value.split_once('\t') else {
        return Err("global backup value must contain exactly one tab".to_owned());
    };
    if project.is_empty() || backup.is_empty() || backup.contains('\t') {
        return Err(
            "global backup value must contain two nonempty tab-separated fields".to_owned(),
        );
    }
    Ok(())
}

fn is_clean_absolute(path: &Path) -> bool {
    path.is_absolute()
        && path.components().all(|component| {
            matches!(
                component,
                Component::Prefix(_) | Component::RootDir | Component::Normal(_)
            )
        })
        && path.components().collect::<PathBuf>() == path
}
