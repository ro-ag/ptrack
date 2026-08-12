use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};

use super::bundle::{BundleError, BundleKind, checked_retained_bytes, validate_path};
use super::sha256::Sha256;

static NEXT_PATH: AtomicU64 = AtomicU64::new(0);
type TestBucket<'a> = (&'a str, u64, Vec<(&'a [u8], &'a [u8])>);

#[test]
fn validates_canonical_project_bundle() {
    let bytes = project_bundle();
    let path = temp_file(&bytes);
    let bundle = validate_path(&path).unwrap();
    assert_eq!(bundle.kind(), BundleKind::Project);
    assert_eq!(bundle.source_format(), 5);
    assert_eq!(bundle.total_records(), 2);
    assert_eq!(bundle.buckets().len(), 10);
    assert_eq!(bundle.buckets()[8].sequence(), 9);
    assert_eq!(bundle.buckets()[5].records()[0].key(), b"meta");
    assert_eq!(bundle.buckets()[5].records()[0].value(), b"meta");
    fs::remove_file(path).unwrap();
}

#[test]
fn rejects_digest_tampering_and_trailing_data() {
    let mut tampered = project_bundle();
    tampered[50] ^= 1;
    assert_invalid(&tampered);
    let mut trailing = project_bundle();
    trailing.push(0);
    assert_invalid(&trailing);
}

#[test]
fn rejects_noncanonical_record_order() {
    assert_invalid(&encode(
        1,
        5,
        &[
            ("capabilities", 0, vec![]),
            ("capability_audits", 0, vec![]),
            ("commits", 0, vec![]),
            ("issues", 0, vec![]),
            ("memory_writebacks", 2, vec![(b"b", b"2"), (b"a", b"1")]),
            ("meta", 0, vec![(b"meta", b"meta")]),
            ("milestones", 0, vec![]),
            ("notes", 0, vec![]),
            ("plans", 0, vec![]),
            ("tasks", 0, vec![]),
        ],
    ));
}

#[test]
fn validates_canonical_global_bundle() {
    let bytes = encode(
        2,
        0,
        &[
            ("backups", 0, vec![]),
            ("config", 0, vec![(b"theme", b"dark")]),
            ("projects", 0, vec![]),
        ],
    );
    let path = temp_file(&bytes);
    let bundle = validate_path(&path).unwrap();
    assert_eq!(bundle.kind(), BundleKind::Global);
    assert_eq!(bundle.total_records(), 1);
    fs::remove_file(path).unwrap();
}

#[test]
fn rejects_nonzero_reserved_fields_and_unsupported_header_values() {
    for (offset, bytes) in [
        (8, 2_u16.to_be_bytes().to_vec()),
        (10, 39_u16.to_be_bytes().to_vec()),
        (12, vec![3]),
        (13, vec![1]),
        (14, 1_u16.to_be_bytes().to_vec()),
        (16, 6_u64.to_be_bytes().to_vec()),
        (24, 14_u32.to_be_bytes().to_vec()),
        (28, 1_u32.to_be_bytes().to_vec()),
        (32, 1_000_001_u64.to_be_bytes().to_vec()),
    ] {
        let mut bundle = project_bundle();
        bundle[offset..offset + bytes.len()].copy_from_slice(&bytes);
        resign(&mut bundle);
        assert_invalid(&bundle);
    }
}

#[test]
fn rejects_noncanonical_bucket_metadata_and_record_limits() {
    for (offset, bytes) in [
        (40, b"NOPE".to_vec()),
        (44, 0_u16.to_be_bytes().to_vec()),
        (46, 1_u16.to_be_bytes().to_vec()),
        (56, 1_000_001_u64.to_be_bytes().to_vec()),
        (64, b"x".to_vec()),
    ] {
        let mut bundle = project_bundle();
        bundle[offset..offset + bytes.len()].copy_from_slice(&bytes);
        resign(&mut bundle);
        assert_invalid(&bundle);
    }
}

#[test]
fn rejects_bad_key_shapes_and_sequences() {
    let numeric = |key: &'static [u8], sequence| {
        encode(
            1,
            1,
            &[
                ("meta", 0, vec![(b"meta", b"meta")]),
                ("notes", 0, vec![]),
                ("plans", sequence, vec![(key, b"plan")]),
                ("tasks", 0, vec![]),
            ],
        )
    };
    assert_invalid(&numeric(b"short", 1));
    assert_invalid(&numeric(b"\0\0\0\0\0\0\0\0", 1));
    assert_invalid(&numeric(b"\0\0\0\0\0\0\0\x02", 1));
    assert_invalid(&encode(
        1,
        1,
        &[
            ("meta", 0, vec![(b"wrong", b"meta")]),
            ("notes", 0, vec![]),
            ("plans", 0, vec![]),
            ("tasks", 0, vec![]),
        ],
    ));
}

#[test]
fn accepts_historical_required_set_and_later_known_buckets() {
    let bytes = encode(
        1,
        1,
        &[
            ("commits", 0, vec![]),
            ("meta", 0, vec![(b"meta", b"meta")]),
            ("notes", 0, vec![]),
            ("plans", 0, vec![]),
            ("tasks", 0, vec![]),
        ],
    );
    let path = temp_file(&bytes);
    assert_eq!(validate_path(&path).unwrap().buckets().len(), 5);
    fs::remove_file(path).unwrap();
}

#[test]
fn rejects_missing_version_required_unknown_and_duplicate_buckets() {
    let base = [
        ("meta", 0, vec![(b"meta".as_slice(), b"meta".as_slice())]),
        ("notes", 0, vec![]),
        ("plans", 0, vec![]),
        ("tasks", 0, vec![]),
    ];
    assert_invalid(&encode(1, 2, &base));
    let mut unknown = base.to_vec();
    unknown.insert(0, ("alien", 0, vec![]));
    assert_invalid(&encode(1, 1, &unknown));
    let mut duplicate = base.to_vec();
    duplicate.insert(2, ("notes", 0, vec![]));
    assert_invalid(&encode(1, 1, &duplicate));
}

#[test]
fn rejects_relative_and_symlink_paths() {
    assert!(validate_path(std::path::Path::new("bundle.bin")).is_err());
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let target = temp_file(&project_bundle());
        let link = target.with_extension("link");
        symlink(&target, &link).unwrap();
        assert!(validate_path(&link).is_err());
        fs::remove_file(link).unwrap();
        fs::remove_file(target).unwrap();
    }
}

#[test]
fn retained_byte_limit_is_checked_without_allocating() {
    assert_eq!(checked_retained_bytes(0, 1, 1).unwrap(), 22);
    assert!(checked_retained_bytes(256 << 20, 1, 0).is_err());
    assert!(checked_retained_bytes(u64::MAX, 1, 0).is_err());
}

fn project_bundle() -> Vec<u8> {
    encode(
        1,
        5,
        &[
            ("capabilities", 0, vec![]),
            ("capability_audits", 0, vec![]),
            ("commits", 0, vec![]),
            ("issues", 0, vec![]),
            ("memory_writebacks", 0, vec![]),
            ("meta", 0, vec![(b"meta", b"meta")]),
            ("milestones", 0, vec![]),
            ("notes", 0, vec![]),
            ("plans", 9, vec![(b"\0\0\0\0\0\0\0\x01", b"plan")]),
            ("tasks", 0, vec![]),
        ],
    )
}

fn encode(kind: u8, source: u64, buckets: &[TestBucket<'_>]) -> Vec<u8> {
    let mut out = Vec::new();
    let records: u64 = buckets.iter().map(|bucket| bucket.2.len() as u64).sum();
    out.extend_from_slice(b"PTRKMIG1");
    out.extend_from_slice(&1_u16.to_be_bytes());
    out.extend_from_slice(&40_u16.to_be_bytes());
    out.push(kind);
    out.push(0);
    out.extend_from_slice(&0_u16.to_be_bytes());
    out.extend_from_slice(&source.to_be_bytes());
    out.extend_from_slice(
        &u32::try_from(buckets.len())
            .expect("test bucket count fits")
            .to_be_bytes(),
    );
    out.extend_from_slice(&0_u32.to_be_bytes());
    out.extend_from_slice(&records.to_be_bytes());
    for (name, sequence, rows) in buckets {
        out.extend_from_slice(b"BUKT");
        out.extend_from_slice(
            &u16::try_from(name.len())
                .expect("test bucket name fits")
                .to_be_bytes(),
        );
        out.extend_from_slice(&0_u16.to_be_bytes());
        out.extend_from_slice(&sequence.to_be_bytes());
        out.extend_from_slice(&(rows.len() as u64).to_be_bytes());
        out.extend_from_slice(name.as_bytes());
        for (key, value) in rows {
            out.extend_from_slice(&(key.len() as u64).to_be_bytes());
            out.extend_from_slice(&(value.len() as u64).to_be_bytes());
            out.extend_from_slice(key);
            out.extend_from_slice(value);
        }
    }
    let mut hash = Sha256::new();
    hash.update(&out);
    let digest = hash.finish();
    out.extend_from_slice(b"HASH");
    out.extend_from_slice(&1_u16.to_be_bytes());
    out.extend_from_slice(&32_u16.to_be_bytes());
    out.extend_from_slice(&digest);
    out
}

fn assert_invalid(bytes: &[u8]) {
    let path = temp_file(bytes);
    assert!(matches!(validate_path(&path), Err(BundleError::Invalid(_))));
    fs::remove_file(path).unwrap();
}

fn resign(bytes: &mut Vec<u8>) {
    bytes.truncate(bytes.len() - 40);
    let mut hash = Sha256::new();
    hash.update(bytes);
    let digest = hash.finish();
    bytes.extend_from_slice(b"HASH");
    bytes.extend_from_slice(&1_u16.to_be_bytes());
    bytes.extend_from_slice(&32_u16.to_be_bytes());
    bytes.extend_from_slice(&digest);
}

fn temp_file(bytes: &[u8]) -> std::path::PathBuf {
    let number = NEXT_PATH.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "ptrack-migrate-{}-{number}.bundle",
        std::process::id()
    ));
    fs::write(&path, bytes).unwrap();
    path
}
