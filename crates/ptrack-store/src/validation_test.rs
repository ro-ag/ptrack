use ptrack_core::{
    Commit, Meta, NativeRecord, ProjectRef, RecordKind, Timestamp, decode_record, encode_record,
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

/// Replaces one same-length byte window in an encoded payload, the way a
/// record written before a field rule existed would differ from a fresh one.
fn splice(payload: &mut [u8], from: &[u8], to: &[u8]) {
    assert_eq!(from.len(), to.len());
    let index = payload
        .windows(from.len())
        .position(|window| window == from)
        .expect("marker bytes in payload");
    payload[index..index + from.len()].copy_from_slice(to);
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
        stack: None,
        scratchpad: None,
        summary_updated_at: None,
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
        stack: None,
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
            stack: None,
        })
    );
}

#[test]
fn raw_global_records_match_the_api_contract() {
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

#[test]
fn commit_payloads_with_unusable_sha_or_multiline_subject_fail_open_validation() {
    let record = |sha: &str, subject: &str| {
        NativeRecord::Commit(Commit {
            id: 1,
            sha: sha.to_owned(),
            subject: subject.to_owned(),
            plan_id: 0,
            task_id: 0,
            created_at: Timestamp::Zero,
            actor: None,
            ulid: None,
        })
    };
    // The write path refuses the bad shapes at encode time.
    assert!(encode_record(&record("zzzzzzzz", "subject")).is_err());
    assert!(encode_record(&record("--output=/tmp/x", "subject")).is_err());
    assert!(encode_record(&record("abcd1234", "line1\nline2")).is_err());
    // A well-formed record still validates end to end.
    let payload = encode_record(&record("abcd1234", "line1Xline2")).unwrap();
    validation::record(
        Collection::Commits,
        &OwnedRecordKey::Id(1),
        &RecordEnvelope::new(NATIVE_CODEC, NATIVE_PAYLOAD_SCHEMA, payload),
    )
    .unwrap();
    // A record stored before the rule existed can hold anything, so open
    // validation is what refuses it: corrupt one field of a well-formed
    // payload in place, exactly as such a record would sit in an old database.
    let mut payload = encode_record(&record("abcd1234", "line1Xline2")).unwrap();
    splice(&mut payload, b"abcd1234", b"zzzzzzzz");
    let error = validation::record(
        Collection::Commits,
        &OwnedRecordKey::Id(1),
        &RecordEnvelope::new(NATIVE_CODEC, NATIVE_PAYLOAD_SCHEMA, payload),
    )
    .unwrap_err();
    assert!(error.contains("commit.sha"), "{error}");

    let mut payload = encode_record(&record("abcd1234", "line1Xline2")).unwrap();
    splice(&mut payload, b"line1Xline2", b"line1\nline2");
    let error = validation::record(
        Collection::Commits,
        &OwnedRecordKey::Id(1),
        &RecordEnvelope::new(NATIVE_CODEC, NATIVE_PAYLOAD_SCHEMA, payload),
    )
    .unwrap_err();
    assert!(error.contains("commit.subject"), "{error}");
}
