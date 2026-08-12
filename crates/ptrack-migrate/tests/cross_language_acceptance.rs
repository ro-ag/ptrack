#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use ptrack_migrate::{BundleKind, Store, StoreKind, import_path, validate_path};
use ptrack_store::{Collection, OwnedRecordKey};

const MARKER_PREFIX: &str = "PTRACK_XLANG_FIXTURE";
const MAX_GO_OUTPUT_BYTES: usize = 3 * 1024 * 1024;
const MAX_FIXTURE_BUNDLE_BYTES: usize = 512 * 1024;
static NEXT_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(1);

#[test]
fn go_bbolt_export_imports_and_reopens_exactly_in_rust() {
    let fixtures = produce_go_fixtures();
    assert_eq!(fixtures.len(), 2);
    let directory = TestDirectory::new();
    for fixture in &fixtures {
        verify_fixture(fixture, &directory);
    }
}

fn produce_go_fixtures() -> Vec<Fixture> {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap();
    let output = Command::new("go")
        .args([
            "test",
            "-run",
            "^TestCrossLanguageFixtures$",
            "-count=1",
            "-v",
            "./tools/db-migrate-export",
        ])
        .current_dir(repository)
        .env("GOTOOLCHAIN", "local")
        .env("GOPROXY", "off")
        .env("GOSUMDB", "off")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "Go fixture producer failed: stdout={} stderr={}",
        bounded_diagnostic(&output.stdout),
        bounded_diagnostic(&output.stderr),
    );
    parse_fixture_markers(&output.stdout).unwrap()
}

fn verify_fixture(fixture: &Fixture, directory: &TestDirectory) {
    let bundle_path = directory.path(&format!("{}.bundle", fixture.kind.name()));
    let destination = directory.path(&format!("{}.redb", fixture.kind.name()));
    fs::write(&bundle_path, &fixture.bundle).unwrap();
    let bundle = validate_path(&bundle_path).unwrap();
    assert_eq!(bundle.kind(), fixture.kind.bundle_kind());
    assert_eq!(bundle.source_format(), fixture.source_format);
    assert_eq!(bundle.buckets().len() as u64, fixture.bucket_count);
    assert_eq!(bundle.total_records(), fixture.record_count);

    let (store, report) = import_path(&bundle_path, &destination).unwrap();
    drop(store);
    let reopened = Store::open_existing(&destination, fixture.kind.store_kind()).unwrap();
    let collections = Collection::for_store(fixture.kind.store_kind()).collect::<Vec<_>>();
    assert_eq!(bundle.buckets().len(), collections.len());
    assert_eq!(report.collections.len(), collections.len());
    assert_eq!(report.record_count, bundle.total_records());
    assert_eq!(
        report
            .collections
            .iter()
            .map(|collection| collection.collection)
            .collect::<Vec<_>>(),
        collections,
    );

    reopened
        .read(|transaction| {
            for collection in collections {
                let bucket = bundle
                    .buckets()
                    .iter()
                    .find(|bucket| bucket.name() == collection.name())
                    .expect("the Go fixture contains every collection");
                let actual = transaction.scan(collection)?;
                assert_eq!(
                    actual.len(),
                    bucket.records().len(),
                    "{}",
                    collection.name()
                );
                verify_records(collection, &actual, bucket.records(), fixture.source_format);

                let report_collection = report
                    .collections
                    .iter()
                    .find(|item| item.collection == collection)
                    .unwrap();
                assert_eq!(
                    report_collection.record_count,
                    bucket.records().len() as u64,
                    "{}",
                    collection.name(),
                );
                if collection.is_sequenced() {
                    assert_eq!(
                        transaction.sequence_high_water(collection)?,
                        bucket.sequence(),
                        "{}",
                        collection.name(),
                    );
                    assert_eq!(report_collection.sequence, Some(bucket.sequence()));
                } else {
                    assert_eq!(report_collection.sequence, None);
                }
            }
            Ok(())
        })
        .unwrap();
}

fn verify_records(
    collection: Collection,
    actual: &[(OwnedRecordKey, ptrack_store::RecordEnvelope)],
    expected: &[ptrack_migrate::Record],
    source_format: u64,
) {
    for ((actual_key, actual_envelope), expected) in actual.iter().zip(expected) {
        assert_eq!(actual_key, &typed_key(collection, expected.key()));
        assert_eq!(actual_envelope.payload(), expected.value());
        assert_eq!(actual_envelope.codec(), collection.legacy_codec());
        assert_eq!(
            u64::from(actual_envelope.payload_schema()),
            source_format,
            "{}",
            collection.name(),
        );
    }
}

#[test]
fn fixture_marker_parser_rejects_malformed_or_oversized_output() {
    assert!(parse_fixture_markers(&vec![b'x'; MAX_GO_OUTPUT_BYTES + 1]).is_err());
    assert!(parse_fixture_markers(b"PTRACK_XLANG_FIXTURE\tproject\t5\t10\t10\t0z\n").is_err());
    assert!(parse_fixture_markers(b"PTRACK_XLANG_FIXTURE\tunknown\t0\t0\t0\t00\n").is_err());
    let oversized_hex = "00".repeat(MAX_FIXTURE_BUNDLE_BYTES + 1);
    let oversized = format!("PTRACK_XLANG_FIXTURE\tproject\t5\t10\t10\t{oversized_hex}\n");
    assert!(parse_fixture_markers(oversized.as_bytes()).is_err());
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FixtureKind {
    Project,
    Global,
}

impl FixtureKind {
    const fn name(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Global => "global",
        }
    }

    const fn bundle_kind(self) -> BundleKind {
        match self {
            Self::Project => BundleKind::Project,
            Self::Global => BundleKind::Global,
        }
    }

    const fn store_kind(self) -> StoreKind {
        match self {
            Self::Project => StoreKind::Project,
            Self::Global => StoreKind::Global,
        }
    }
}

struct Fixture {
    kind: FixtureKind,
    source_format: u64,
    bucket_count: u64,
    record_count: u64,
    bundle: Vec<u8>,
}

fn parse_fixture_markers(output: &[u8]) -> Result<Vec<Fixture>, String> {
    if output.len() > MAX_GO_OUTPUT_BYTES {
        return Err("Go fixture output exceeds its bound".to_owned());
    }
    let output = std::str::from_utf8(output).map_err(|_| "Go fixture output is not UTF-8")?;
    let mut fixtures = Vec::new();
    for line in output.lines() {
        if !line.starts_with(MARKER_PREFIX) {
            continue;
        }
        let fields = line.split('\t').collect::<Vec<_>>();
        if fields.len() != 6 || fields[0] != MARKER_PREFIX {
            return Err("malformed Go fixture marker".to_owned());
        }
        let kind = match fields[1] {
            "project" => FixtureKind::Project,
            "global" => FixtureKind::Global,
            _ => return Err("unknown Go fixture kind".to_owned()),
        };
        if fixtures
            .iter()
            .any(|fixture: &Fixture| fixture.kind == kind)
        {
            return Err("duplicate Go fixture kind".to_owned());
        }
        let source_format = parse_decimal(fields[2], "source format")?;
        let bucket_count = parse_decimal(fields[3], "bucket count")?;
        let record_count = parse_decimal(fields[4], "record count")?;
        let bundle = decode_hex(fields[5])?;
        fixtures.push(Fixture {
            kind,
            source_format,
            bucket_count,
            record_count,
            bundle,
        });
    }
    if fixtures.len() != 2
        || !fixtures
            .iter()
            .any(|fixture| fixture.kind == FixtureKind::Project)
        || !fixtures
            .iter()
            .any(|fixture| fixture.kind == FixtureKind::Global)
    {
        return Err("Go fixture output must contain one project and one global marker".to_owned());
    }
    Ok(fixtures)
}

fn parse_decimal(value: &str, name: &str) -> Result<u64, String> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!("invalid fixture {name}"));
    }
    value
        .parse()
        .map_err(|_| format!("fixture {name} is out of range"))
}

fn decode_hex(encoded: &str) -> Result<Vec<u8>, String> {
    if encoded.len() > MAX_FIXTURE_BUNDLE_BYTES * 2 {
        return Err("fixture bundle exceeds its bound".to_owned());
    }
    if encoded.is_empty() || !encoded.len().is_multiple_of(2) {
        return Err("fixture bundle has invalid hex length".to_owned());
    }
    encoded
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| Ok((hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?))
        .collect()
}

fn hex_nibble(byte: u8) -> Result<u8, String> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        _ => Err("fixture bundle contains non-lowercase-hex data".to_owned()),
    }
}

fn typed_key(collection: Collection, key: &[u8]) -> OwnedRecordKey {
    match collection {
        Collection::ProjectMeta => {
            assert_eq!(key, b"meta");
            OwnedRecordKey::Singleton
        }
        Collection::Plans
        | Collection::Tasks
        | Collection::Notes
        | Collection::Milestones
        | Collection::Issues
        | Collection::Commits
        | Collection::Capabilities
        | Collection::CapabilityAudits => {
            OwnedRecordKey::Id(u64::from_be_bytes(key.try_into().unwrap()))
        }
        Collection::MemoryWritebacks
        | Collection::GlobalConfig
        | Collection::GlobalProjects
        | Collection::GlobalBackups => OwnedRecordKey::Bytes(key.to_vec()),
    }
}

fn bounded_diagnostic(bytes: &[u8]) -> String {
    const LIMIT: usize = 4 * 1024;
    String::from_utf8_lossy(&bytes[..bytes.len().min(LIMIT)]).into_owned()
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let number = NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ptrack-cross-language-{}-{number}",
            std::process::id(),
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
