use std::error::Error;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, Read};
use std::path::Path;

use crate::sha256::Sha256;

const MAGIC: &[u8; 8] = b"PTRKMIG1";
const VERSION: u16 = 1;
const HEADER_LEN: u16 = 40;
const TRAILER_LEN: u64 = 40;
const MAX_BUNDLE_LEN: u64 = 16 * 1024 * 1024 * 1024;
const MAX_BUCKETS: u32 = 13;
const MAX_RECORDS: u64 = 1_000_000;
const MAX_KEY_LEN: u64 = 1024 * 1024;
const MAX_VALUE_LEN: u64 = 256 * 1024 * 1024;
const RECORD_ENVELOPE_OVERHEAD: u64 = 20;
const MAX_RETAINED_BYTES: u64 = 256 * 1024 * 1024;
const PROJECT_BUCKETS: [&str; 10] = [
    "capabilities",
    "capability_audits",
    "commits",
    "issues",
    "memory_writebacks",
    "meta",
    "milestones",
    "notes",
    "plans",
    "tasks",
];
const GLOBAL_BUCKETS: [&str; 3] = ["backups", "config", "projects"];

/// The legacy database family contained by a migration bundle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BundleKind {
    Project,
    Global,
}

impl fmt::Display for BundleKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Project => "project",
            Self::Global => "global",
        })
    }
}

/// One raw legacy record covered by the validated bundle digest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Record {
    key: Vec<u8>,
    value: Vec<u8>,
}

impl Record {
    #[must_use]
    pub fn key(&self) -> &[u8] {
        &self.key
    }

    #[must_use]
    pub fn value(&self) -> &[u8] {
        &self.value
    }
}

/// An integrity-checked canonical legacy bucket and its ordered raw records.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Bucket {
    name: String,
    sequence: u64,
    records: Vec<Record>,
}

impl Bucket {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    #[must_use]
    pub fn records(&self) -> &[Record] {
        &self.records
    }
}

/// A fully parsed bundle whose SHA-256 integrity digest was verified.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedBundle {
    kind: BundleKind,
    source_format: u64,
    total_records: u64,
    byte_len: u64,
    buckets: Vec<Bucket>,
    sha256: [u8; 32],
}

impl ValidatedBundle {
    #[must_use]
    pub const fn kind(&self) -> BundleKind {
        self.kind
    }

    #[must_use]
    pub const fn source_format(&self) -> u64 {
        self.source_format
    }

    #[must_use]
    pub const fn total_records(&self) -> u64 {
        self.total_records
    }

    #[must_use]
    pub const fn byte_len(&self) -> u64 {
        self.byte_len
    }

    #[must_use]
    pub fn buckets(&self) -> &[Bucket] {
        &self.buckets
    }

    #[must_use]
    pub const fn sha256(&self) -> &[u8; 32] {
        &self.sha256
    }
}

#[derive(Debug)]
pub enum BundleError {
    Io(io::Error),
    Invalid(String),
}

impl fmt::Display for BundleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "cannot read migration bundle: {error}"),
            Self::Invalid(message) => write!(formatter, "invalid migration bundle: {message}"),
        }
    }
}

impl Error for BundleError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Invalid(_) => None,
        }
    }
}

impl From<io::Error> for BundleError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

/// Opens and validates a named bundle without writing to it or any database.
///
/// # Errors
///
/// Returns [`BundleError`] when the path is unsafe, reading fails, or any byte
/// violates the canonical bundle contract or its integrity digest.
pub fn validate_path(path: &Path) -> Result<ValidatedBundle, BundleError> {
    if !path.is_absolute() {
        return invalid("bundle path must be absolute");
    }
    let before = std::fs::symlink_metadata(path)?;
    if before.file_type().is_symlink() {
        return invalid("bundle path must not be a symbolic link");
    }
    if !before.is_file() {
        return invalid("bundle path must name a regular file");
    }
    let file = open_bundle(path)?;
    let opened = file.metadata()?;
    let after = std::fs::symlink_metadata(path)?;
    if after.file_type().is_symlink()
        || !after.is_file()
        || !opened.is_file()
        || !same_file(&before, &opened)
        || !same_file(&after, &opened)
    {
        return invalid("bundle path changed while it was opened");
    }
    parse(BufReader::new(file), opened.len())
}

#[cfg(unix)]
fn same_file(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(windows)]
fn same_file(_left: &std::fs::Metadata, _right: &std::fs::Metadata) -> bool {
    // Stable Rust does not yet expose Windows volume serial + file index.
    // Length and timestamps are not identity, so fail closed instead.
    false
}

#[cfg(not(any(unix, windows)))]
fn same_file(_left: &std::fs::Metadata, _right: &std::fs::Metadata) -> bool {
    false
}

#[cfg(windows)]
fn open_bundle(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;

    OpenOptions::new().read(true).share_mode(0).open(path)
}

#[cfg(not(windows))]
fn open_bundle(path: &Path) -> io::Result<File> {
    OpenOptions::new().read(true).open(path)
}

#[allow(clippy::too_many_lines)]
fn parse(reader: impl Read, byte_len: u64) -> Result<ValidatedBundle, BundleError> {
    if !(u64::from(HEADER_LEN) + TRAILER_LEN..=MAX_BUNDLE_LEN).contains(&byte_len) {
        return invalid("bundle length is outside the supported range");
    }
    let mut input = Input {
        reader,
        hash: Sha256::new(),
        remaining: byte_len,
    };
    let magic = input.hashed_array::<8>()?;
    require(magic == *MAGIC, "bad magic")?;
    require(input.hashed_u16()? == VERSION, "unsupported bundle version")?;
    require(input.hashed_u16()? == HEADER_LEN, "bad header length")?;
    let kind = match input.hashed_u8()? {
        1 => BundleKind::Project,
        2 => BundleKind::Global,
        _ => return invalid("unknown bundle kind"),
    };
    require(input.hashed_u8()? == 0, "header flags must be zero")?;
    require(
        input.hashed_u16()? == 0,
        "header reserved field must be zero",
    )?;
    let source_format = input.hashed_u64()?;
    match kind {
        BundleKind::Project => require(source_format <= 5, "unsupported project source format")?,
        BundleKind::Global => require(source_format == 0, "global source format must be zero")?,
    }
    let bucket_count = input.hashed_u32()?;
    require(bucket_count <= MAX_BUCKETS, "too many buckets")?;
    require(
        input.hashed_u32()? == 0,
        "header reserved field must be zero",
    )?;
    let declared_records = input.hashed_u64()?;
    require(declared_records <= MAX_RECORDS, "too many records")?;

    if kind == BundleKind::Global {
        require(
            usize::try_from(bucket_count).ok() == Some(GLOBAL_BUCKETS.len()),
            "global bucket count does not match the canonical set",
        )?;
    }
    let mut buckets: Vec<Bucket> = Vec::with_capacity(
        usize::try_from(bucket_count).expect("bucket count is bounded by thirteen"),
    );
    let mut actual_records = 0_u64;
    let mut retained_bytes = 0_u64;
    for _ in 0..bucket_count {
        require(
            input.hashed_array::<4>()? == *b"BUKT",
            "missing BUKT section",
        )?;
        let name_len = input.hashed_u16()?;
        require((1..=255).contains(&name_len), "invalid bucket name length")?;
        require(input.hashed_u16()? == 0, "bucket flags must be zero")?;
        let sequence = input.hashed_u64()?;
        let record_count = input.hashed_u64()?;
        require(record_count <= MAX_RECORDS, "too many records in bucket")?;
        actual_records = actual_records
            .checked_add(record_count)
            .ok_or_else(|| BundleError::Invalid("record count overflow".to_owned()))?;
        require(
            actual_records <= declared_records,
            "bucket record counts exceed header total",
        )?;
        let name_bytes = input.hashed_vec(u64::from(name_len))?;
        let name = std::str::from_utf8(&name_bytes)
            .map_err(|_| BundleError::Invalid("bucket name is not UTF-8".to_owned()))?;
        require(is_known_bucket(kind, name), "unknown bucket name")?;
        if let Some(previous) = buckets.last() {
            require(
                previous.name.as_str() < name,
                "bucket names are not strictly ordered",
            )?;
        }
        if !is_sequenced(name) {
            require(sequence == 0, "unsequenced bucket has a nonzero sequence")?;
        }
        let mut records: Vec<Record> = Vec::new();
        for _ in 0..record_count {
            let key_len = input.hashed_u64()?;
            let value_len = input.hashed_u64()?;
            require(
                (1..=MAX_KEY_LEN).contains(&key_len),
                "invalid record key length",
            )?;
            require(value_len <= MAX_VALUE_LEN, "record value is too large")?;
            retained_bytes = checked_retained_bytes(retained_bytes, key_len, value_len)?;
            let key = input.hashed_vec(key_len)?;
            if let Some(previous) = records.last() {
                require(
                    previous.key.as_slice() < key.as_slice(),
                    "record keys are not strictly ordered",
                )?;
            }
            let value = input.hashed_vec(value_len)?;
            records.push(Record { key, value });
        }
        validate_bucket_records(name, sequence, &records)?;
        buckets.push(Bucket {
            name: name.to_owned(),
            sequence,
            records,
        });
    }
    validate_bucket_coverage(kind, source_format, &buckets)?;
    require(
        actual_records == declared_records,
        "record count does not match header total",
    )?;
    require(
        input.remaining == TRAILER_LEN,
        "unexpected bytes before HASH trailer",
    )?;
    require(input.raw_array::<4>()? == *b"HASH", "missing HASH trailer")?;
    require(input.raw_u16()? == 1, "unsupported hash algorithm")?;
    require(input.raw_u16()? == 32, "bad digest length")?;
    let expected_digest = input.raw_array::<32>()?;
    require(input.remaining == 0, "trailing bytes after HASH trailer")?;
    let digest = input.hash.finish();
    require(digest == expected_digest, "SHA-256 digest mismatch")?;
    Ok(ValidatedBundle {
        kind,
        source_format,
        total_records: declared_records,
        byte_len,
        buckets,
        sha256: digest,
    })
}

pub(crate) fn checked_retained_bytes(
    current: u64,
    key: u64,
    value: u64,
) -> Result<u64, BundleError> {
    let next = current
        .checked_add(key)
        .and_then(|total| total.checked_add(value))
        .and_then(|total| total.checked_add(RECORD_ENVELOPE_OVERHEAD))
        .ok_or_else(|| BundleError::Invalid("retained record bytes overflow".to_owned()))?;
    require(
        next <= MAX_RETAINED_BYTES,
        "retained record bytes exceed the in-memory import limit",
    )?;
    Ok(next)
}

fn validate_bucket_records(
    name: &str,
    sequence: u64,
    records: &[Record],
) -> Result<(), BundleError> {
    if name == "meta" {
        require(
            records.len() == 1 && records[0].key == b"meta",
            "project meta must contain exactly the singleton meta key",
        )?;
    }
    if is_numeric_keyed(name) {
        let mut maximum = 0_u64;
        for record in records {
            require(
                record.key.len() == 8,
                "numeric record key must be eight bytes",
            )?;
            let id = u64::from_be_bytes(
                record
                    .key
                    .as_slice()
                    .try_into()
                    .expect("numeric key length was checked"),
            );
            require(id != 0, "numeric record key must be nonzero")?;
            maximum = maximum.max(id);
        }
        require(
            sequence >= maximum,
            "bucket sequence is below its maximum ID",
        )?;
    }
    Ok(())
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

fn is_known_bucket(kind: BundleKind, name: &str) -> bool {
    match kind {
        BundleKind::Project => PROJECT_BUCKETS.binary_search(&name).is_ok(),
        BundleKind::Global => GLOBAL_BUCKETS.binary_search(&name).is_ok(),
    }
}

fn validate_bucket_coverage(
    kind: BundleKind,
    source_format: u64,
    buckets: &[Bucket],
) -> Result<(), BundleError> {
    let required = match kind {
        BundleKind::Global => GLOBAL_BUCKETS.as_slice(),
        BundleKind::Project => PROJECT_BUCKETS.as_slice(),
    };
    for name in required {
        if kind == BundleKind::Project && introduced_in(name) > source_format {
            continue;
        }
        require(
            buckets.iter().any(|bucket| bucket.name == *name),
            "required bucket is missing",
        )?;
    }
    Ok(())
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

struct Input<R> {
    reader: R,
    hash: Sha256,
    remaining: u64,
}

impl<R: Read> Input<R> {
    fn read(&mut self, target: &mut [u8], hashed: bool) -> Result<(), BundleError> {
        let length = u64::try_from(target.len()).expect("usize fits in u64");
        if length > self.remaining {
            return invalid("truncated bundle");
        }
        self.reader.read_exact(target)?;
        self.remaining -= length;
        if hashed {
            self.hash.update(target);
        }
        Ok(())
    }

    fn hashed_array<const N: usize>(&mut self) -> Result<[u8; N], BundleError> {
        let mut bytes = [0; N];
        self.read(&mut bytes, true)?;
        Ok(bytes)
    }

    fn raw_array<const N: usize>(&mut self) -> Result<[u8; N], BundleError> {
        let mut bytes = [0; N];
        self.read(&mut bytes, false)?;
        Ok(bytes)
    }

    fn hashed_u8(&mut self) -> Result<u8, BundleError> {
        Ok(self.hashed_array::<1>()?[0])
    }

    fn hashed_u16(&mut self) -> Result<u16, BundleError> {
        Ok(u16::from_be_bytes(self.hashed_array()?))
    }

    fn raw_u16(&mut self) -> Result<u16, BundleError> {
        Ok(u16::from_be_bytes(self.raw_array()?))
    }

    fn hashed_u32(&mut self) -> Result<u32, BundleError> {
        Ok(u32::from_be_bytes(self.hashed_array()?))
    }

    fn hashed_u64(&mut self) -> Result<u64, BundleError> {
        Ok(u64::from_be_bytes(self.hashed_array()?))
    }

    fn hashed_vec(&mut self, length: u64) -> Result<Vec<u8>, BundleError> {
        if length > self.remaining {
            return invalid("truncated bundle");
        }
        let length = usize::try_from(length).map_err(|_| {
            BundleError::Invalid("field length does not fit this platform".to_owned())
        })?;
        let mut bytes = vec![0; length];
        self.read(&mut bytes, true)?;
        Ok(bytes)
    }
}

fn require(condition: bool, message: &str) -> Result<(), BundleError> {
    if condition { Ok(()) } else { invalid(message) }
}

fn invalid<T>(message: &str) -> Result<T, BundleError> {
    Err(BundleError::Invalid(message.to_owned()))
}
