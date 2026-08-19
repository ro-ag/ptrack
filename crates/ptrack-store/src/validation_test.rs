use ptrack_core::{
    Meta, NativeRecord, ProjectRef, RecordKind, Timestamp, decode_record, encode_record,
};

use super::{
    Collection, LEGACY_CODEC_RAW, NATIVE_CODEC, NATIVE_PAYLOAD_SCHEMA, OwnedRecordKey,
    RecordEnvelope,
};
use crate::validation;

fn native(record: NativeRecord) -> RecordEnvelope {
    RecordEnvelope::new(
        NATIVE_CODEC,
        NATIVE_PAYLOAD_SCHEMA,
        encode_record(&record).unwrap(),
    )
}

#[test]
fn store_validation_binds_native_payloads_to_collection_keys() {
    let meta = native(NativeRecord::Meta(Meta {
        goal: "goal".to_owned(),
        summary: String::new(),
        active_plan: 0,
        created_at: Timestamp::Zero,
        updated_at: Timestamp::Zero,
        format_version: 5,
        last_write_version: "v0.21.0".to_owned(),
        active_plans: Vec::new(),
        actors: Vec::new(),
    }));
    validation::record(Collection::ProjectMeta, &OwnedRecordKey::Singleton, &meta).unwrap();
    assert!(validation::record(Collection::Plans, &OwnedRecordKey::Id(1), &meta).is_err());

    let project_path = std::env::temp_dir().join("project");
    let project_path = project_path.to_string_lossy().into_owned();
    let other_path = std::env::temp_dir().join("other");
    let other_path = other_path.to_string_lossy().into_owned();
    let project = native(NativeRecord::ProjectRef(ProjectRef {
        name: "project".to_owned(),
        path: project_path.clone(),
        last_seen: Timestamp::Zero,
    }));
    validation::record(
        Collection::GlobalProjects,
        &OwnedRecordKey::Bytes(project_path.as_bytes().to_vec()),
        &project,
    )
    .unwrap();
    assert!(
        validation::record(
            Collection::GlobalProjects,
            &OwnedRecordKey::Bytes(other_path.as_bytes().to_vec()),
            &project,
        )
        .is_err()
    );
    assert_eq!(
        decode_record(RecordKind::ProjectRef, project.payload()).unwrap(),
        NativeRecord::ProjectRef(ProjectRef {
            name: "project".to_owned(),
            path: project_path,
            last_seen: Timestamp::Zero,
        })
    );
}

#[test]
fn raw_global_records_match_the_go_api_contract() {
    validation::record(
        Collection::GlobalConfig,
        &OwnedRecordKey::Bytes(vec![0xff]),
        &RecordEnvelope::new(LEGACY_CODEC_RAW, 0, vec![0xfe]),
    )
    .unwrap();
    validation::record(
        Collection::GlobalBackups,
        &OwnedRecordKey::Bytes(b"1700000000".to_vec()),
        &RecordEnvelope::new(LEGACY_CODEC_RAW, 0, b"relative\t../unclean".to_vec()),
    )
    .unwrap();
    assert!(
        validation::record(
            Collection::GlobalBackups,
            &OwnedRecordKey::Bytes(b"01700000000".to_vec()),
            &RecordEnvelope::new(LEGACY_CODEC_RAW, 0, b"left\tright".to_vec()),
        )
        .is_err()
    );
    assert!(
        validation::record(
            Collection::GlobalBackups,
            &OwnedRecordKey::Bytes(b"1".to_vec()),
            &RecordEnvelope::new(LEGACY_CODEC_RAW, 0, b"left\tright\textra".to_vec()),
        )
        .is_err()
    );
}
