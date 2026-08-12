use super::{
    Collection, LEGACY_CODEC_GO_GOB, LEGACY_CODEC_RAW, OwnedRecordKey, RecordKey, StoreError,
    StoreKind,
};
use crate::schema::{ALL_COLLECTIONS, collections_for, decode_key};

#[test]
fn schema_contains_the_exact_legacy_collection_set() {
    assert_eq!(ALL_COLLECTIONS.len(), 13);
    assert_eq!(collections_for(StoreKind::Project).count(), 10);
    assert_eq!(collections_for(StoreKind::Global).count(), 3);
    assert_eq!(
        ALL_COLLECTIONS
            .iter()
            .filter(|item| item.is_sequenced())
            .count(),
        9
    );
    for collection in Collection::all() {
        assert_eq!(
            Collection::from_legacy_name(collection.name().as_bytes()),
            Some(*collection)
        );
    }
    assert_eq!(Collection::from_legacy_name(b"Tasks"), None);
    assert_eq!(Collection::GlobalConfig.legacy_codec(), LEGACY_CODEC_RAW);
    assert_eq!(Collection::GlobalBackups.legacy_codec(), LEGACY_CODEC_RAW);
    assert_eq!(Collection::Tasks.legacy_codec(), LEGACY_CODEC_GO_GOB);
    assert_eq!(
        Collection::GlobalProjects.legacy_codec(),
        LEGACY_CODEC_GO_GOB
    );
}

#[test]
fn keys_use_stable_binary_representations() {
    assert_eq!(
        RecordKey::Singleton
            .encode(Collection::ProjectMeta)
            .unwrap(),
        b"meta"
    );
    assert_eq!(
        RecordKey::Id(0x0102_0304_0506_0708)
            .encode(Collection::Tasks)
            .unwrap(),
        [1, 2, 3, 4, 5, 6, 7, 8]
    );
    assert_eq!(
        RecordKey::Bytes(&[0, 0xff])
            .encode(Collection::MemoryWritebacks)
            .unwrap(),
        [0, 0xff]
    );
    let owned = OwnedRecordKey::Bytes(vec![0, 0xff]);
    assert_eq!(owned.as_borrowed(), RecordKey::Bytes(&[0, 0xff]));
}

#[test]
fn stored_keys_decode_strictly() {
    assert_eq!(
        decode_key(Collection::Tasks, &42_u64.to_be_bytes()).unwrap(),
        OwnedRecordKey::Id(42)
    );
    assert!(matches!(
        decode_key(Collection::Tasks, &[0; 7]),
        Err(StoreError::InvalidManifest(_))
    ));
    assert!(matches!(
        decode_key(Collection::ProjectMeta, b"wrong"),
        Err(StoreError::InvalidManifest(_))
    ));
}

#[test]
fn collection_and_key_mismatches_are_rejected() {
    assert!(matches!(
        Collection::Tasks.validate_store(StoreKind::Global),
        Err(StoreError::CollectionStoreMismatch { .. })
    ));
    assert!(matches!(
        RecordKey::Bytes(b"1").encode(Collection::Tasks),
        Err(StoreError::KeyKindMismatch { .. })
    ));
}
