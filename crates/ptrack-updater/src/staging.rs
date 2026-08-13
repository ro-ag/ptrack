use std::collections::BTreeMap;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Seek, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use futures_util::StreamExt;
use reqwest::header::{ACCEPT, CONTENT_LENGTH, USER_AGENT};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::discovery::{
    Asset, Candidate, Client, Target, UpdateError, package_name, parse_version, validate_asset_url,
};
use crate::permissions::{
    create_private_dir, create_private_regular, open_private_regular, prepare_private_dir,
    secure_private_path, validate_private_path,
};

const MAX_ARCHIVE_ENTRY_BYTES: u64 = 128 << 20;
const MAX_ARCHIVE_TOTAL_BYTES: u64 = 160 << 20;
const MAX_CHECKSUM_LINES: usize = 256;
const MAX_CHECKSUM_LINE_BYTES: usize = 1024;
const MAX_ASSET_DOWNLOAD_TIME: Duration = Duration::from_secs(10 * 60);
const MAX_MANIFEST_BYTES: u64 = 1 << 20;
const MAX_ASSET_BYTES: u64 = 512 << 20;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum StageKind {
    DarwinDmg,
    LinuxBinary,
    WindowsZip,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Progress {
    pub asset: String,
    pub downloaded: u64,
    pub total: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StagedUpdate {
    pub root: PathBuf,
    pub asset_path: PathBuf,
    pub payload_path: PathBuf,
    pub state_path: PathBuf,
    pub version: String,
    pub asset_name: String,
    pub os: String,
    pub arch: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub payload_sha256: String,
    pub payload_size_bytes: u64,
    pub kind: StageKind,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct StageRecord {
    version: String,
    asset_name: String,
    goos: String,
    goarch: String,
    sha256: String,
    size_bytes: u64,
    payload_sha256: String,
    payload_size_bytes: u64,
    kind: StageKind,
}

impl Client {
    /// Downloads and verifies one exact release into a private durable stage.
    ///
    /// # Errors
    /// Returns a cancellation, network, filesystem, checksum, archive, or target error.
    pub async fn stage(
        &self,
        cancellation: &CancellationToken,
        candidate: &Candidate,
        target: &Target,
        base_dir: &Path,
        progress: Option<&(dyn Fn(Progress) + Send + Sync)>,
    ) -> Result<StagedUpdate, UpdateError> {
        validate_candidate(candidate, target)?;
        let root = make_stage_root(base_dir)?;
        let result = self
            .stage_in(cancellation, candidate, target, &root, progress)
            .await;
        if result.is_err() {
            let _ = fs::remove_dir_all(&root);
        }
        result
    }

    async fn stage_in(
        &self,
        cancellation: &CancellationToken,
        candidate: &Candidate,
        target: &Target,
        root: &Path,
        progress: Option<&(dyn Fn(Progress) + Send + Sync)>,
    ) -> Result<StagedUpdate, UpdateError> {
        let manifest_path = root.join("checksums.txt");
        self.download(
            cancellation,
            &candidate.checksums,
            &manifest_path,
            "checksums",
            progress,
        )
        .await?;
        let wanted_digest = checksum_for(&manifest_path, &candidate.package.name)?;

        let asset_path = root.join(&candidate.package.name);
        let (digest, size) = self
            .download(
                cancellation,
                &candidate.package,
                &asset_path,
                "package",
                progress,
            )
            .await?;
        if digest != wanted_digest {
            return Err(UpdateError::InvalidStage);
        }

        let (kind, payload_path) = match target.os.as_str() {
            "darwin" => (StageKind::DarwinDmg, asset_path.clone()),
            "linux" => {
                let path = root.join("ptrack");
                extract_tar_payload(cancellation, &asset_path, &path)?;
                (StageKind::LinuxBinary, path)
            }
            "windows" => {
                let path = root.join("ptrack.exe");
                extract_zip_payload(cancellation, &asset_path, &path)?;
                (StageKind::WindowsZip, path)
            }
            _ => return Err(UpdateError::UnsupportedTarget),
        };
        let mut stage = StagedUpdate {
            root: root.to_path_buf(),
            asset_path,
            payload_path,
            state_path: root.join("state.json"),
            version: candidate.version.clone(),
            asset_name: candidate.package.name.clone(),
            os: target.os.clone(),
            arch: target.arch.clone(),
            sha256: digest,
            size_bytes: size,
            payload_sha256: String::new(),
            payload_size_bytes: 0,
            kind,
        };
        validate_payload_machine(cancellation, &stage)?;
        let limit = if kind == StageKind::DarwinDmg {
            MAX_ASSET_BYTES
        } else {
            MAX_ARCHIVE_ENTRY_BYTES
        };
        (stage.payload_sha256, stage.payload_size_bytes) =
            hash_regular_file(cancellation, &stage.payload_path, limit)?;
        write_stage_record(&stage)?;
        Ok(stage)
    }

    async fn download(
        &self,
        cancellation: &CancellationToken,
        asset: &Asset,
        destination: &Path,
        progress_name: &str,
        progress: Option<&(dyn Fn(Progress) + Send + Sync)>,
    ) -> Result<(String, u64), UpdateError> {
        let production_initial =
            reqwest::Url::parse(&asset.download_url).map_err(|_| UpdateError::InvalidStage)?;
        #[cfg(test)]
        let test_transport = self.test_asset_transport();
        #[cfg(not(test))]
        let test_transport: Option<(&str, reqwest::Client)> = None;
        let (initial, http, validate_final) = if let Some((base, client)) = test_transport {
            let url =
                reqwest::Url::parse(&format!("{}/{progress_name}", base.trim_end_matches('/')))
                    .map_err(|_| UpdateError::InvalidStage)?;
            (url, client, false)
        } else {
            let redirect_initial = production_initial.clone();
            let client = reqwest::Client::builder()
                .timeout(MAX_ASSET_DOWNLOAD_TIME)
                .redirect(reqwest::redirect::Policy::custom(move |attempt| {
                    if attempt.previous().len() >= 3 {
                        return attempt.error("too many asset redirects");
                    }
                    if validate_download_url(attempt.url(), &redirect_initial).is_err() {
                        return attempt.error("unsafe asset redirect");
                    }
                    attempt.follow()
                }))
                .build()
                .map_err(|_| UpdateError::InvalidStage)?;
            (production_initial.clone(), client, true)
        };
        let request = http
            .get(initial.clone())
            .header(ACCEPT, "application/octet-stream")
            .header(USER_AGENT, "p-track-updater")
            .build()
            .map_err(|_| UpdateError::InvalidStage)?;
        let response = tokio::select! {
            () = cancellation.cancelled() => return Err(UpdateError::Cancelled),
            response = http.execute(request) => response.map_err(|_| UpdateError::InvalidStage)?,
        };
        if response.status() != reqwest::StatusCode::OK
            || (validate_final
                && validate_download_url(response.url(), &production_initial).is_err())
        {
            return Err(UpdateError::InvalidStage);
        }
        if let Some(length) = response.headers().get(CONTENT_LENGTH)
            && length
                .to_str()
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                != Some(asset.size_bytes)
        {
            return Err(UpdateError::InvalidStage);
        }
        let mut file =
            create_private_regular(destination).map_err(|_| UpdateError::InvalidStage)?;
        let mut stream = response.bytes_stream();
        let mut hash = Sha256::new();
        let mut downloaded = 0_u64;
        while let Some(chunk) = tokio::select! {
            () = cancellation.cancelled() => {
                drop(file);
                let _ = fs::remove_file(destination);
                return Err(UpdateError::Cancelled);
            }
            chunk = stream.next() => chunk,
        } {
            let chunk = chunk.map_err(|_| UpdateError::InvalidStage)?;
            let chunk_len = u64::try_from(chunk.len()).map_err(|_| UpdateError::InvalidStage)?;
            downloaded = downloaded
                .checked_add(chunk_len)
                .filter(|size| *size <= asset.size_bytes)
                .ok_or(UpdateError::InvalidStage)?;
            file.write_all(&chunk)
                .map_err(|_| UpdateError::InvalidStage)?;
            hash.update(&chunk);
            if let Some(notify) = progress {
                notify(Progress {
                    asset: progress_name.to_owned(),
                    downloaded,
                    total: asset.size_bytes,
                });
            }
        }
        if downloaded != asset.size_bytes {
            drop(file);
            let _ = fs::remove_file(destination);
            return Err(UpdateError::InvalidStage);
        }
        file.sync_all().map_err(|_| UpdateError::InvalidStage)?;
        drop(file);
        secure_private_path(destination, false).map_err(|_| UpdateError::InvalidStage)?;
        Ok((hex_lower(&hash.finalize()), downloaded))
    }
}

fn validate_candidate(candidate: &Candidate, target: &Target) -> Result<(), UpdateError> {
    let expected = package_name(target, &candidate.version)?;
    if parse_version(&candidate.version, true)?.to_string() != candidate.version
        || candidate.tag != format!("v{}", candidate.version)
        || candidate.package.name != expected
        || candidate.checksums.name != "checksums.txt"
        || candidate.package.size_bytes == 0
        || candidate.package.size_bytes > MAX_ASSET_BYTES
        || candidate.checksums.size_bytes == 0
        || candidate.checksums.size_bytes > MAX_MANIFEST_BYTES
    {
        return Err(UpdateError::InvalidStage);
    }
    validate_asset_url(
        &candidate.package.download_url,
        &candidate.tag,
        &candidate.package.name,
    )
    .map_err(|_| UpdateError::InvalidStage)?;
    validate_asset_url(
        &candidate.checksums.download_url,
        &candidate.tag,
        "checksums.txt",
    )
    .map_err(|_| UpdateError::InvalidStage)
}

fn make_stage_root(base_dir: &Path) -> Result<PathBuf, UpdateError> {
    if !base_dir.is_absolute() {
        return Err(UpdateError::InvalidStage);
    }
    prepare_private_dir(base_dir).map_err(|_| UpdateError::InvalidStage)?;
    for _ in 0..32 {
        let mut random = [0_u8; 16];
        getrandom::fill(&mut random).map_err(|_| UpdateError::InvalidStage)?;
        let root = base_dir.join(format!(".stage-{}", hex_lower(&random)));
        match create_private_dir(&root) {
            Ok(()) => {
                return Ok(root);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err(UpdateError::InvalidStage),
        }
    }
    Err(UpdateError::InvalidStage)
}

/// Revalidates a staged update immediately before installation.
///
/// # Errors
/// Rejects any path, record, target, permission, size, digest, or payload change.
pub fn validate_stage(
    cancellation: &CancellationToken,
    stage: &StagedUpdate,
) -> Result<(), UpdateError> {
    check_cancel(cancellation)?;
    if !stage.root.is_absolute()
        || !path_within(&stage.root, &stage.asset_path)
        || !path_within(&stage.root, &stage.payload_path)
        || !path_within(&stage.root, &stage.state_path)
    {
        return Err(UpdateError::InvalidStage);
    }
    validate_private_path(&stage.root, true).map_err(|_| UpdateError::InvalidStage)?;
    for path in [&stage.asset_path, &stage.payload_path, &stage.state_path] {
        validate_private_path(path, false).map_err(|_| UpdateError::InvalidStage)?;
    }
    let payload_limit = if stage.kind == StageKind::DarwinDmg {
        MAX_ASSET_BYTES
    } else {
        MAX_ARCHIVE_ENTRY_BYTES
    };
    let expected_name = package_name(
        &Target {
            os: stage.os.clone(),
            arch: stage.arch.clone(),
        },
        &stage.version,
    )?;
    if parse_version(&stage.version, true)?.to_string() != stage.version
        || stage.asset_name != expected_name
        || stage.asset_path != stage.root.join(&expected_name)
        || stage.state_path != stage.root.join("state.json")
        || stage.payload_path != expected_payload(stage)
        || stage.size_bytes == 0
        || stage.size_bytes > MAX_ASSET_BYTES
        || stage.payload_size_bytes == 0
        || stage.payload_size_bytes > payload_limit
        || !valid_digest(&stage.sha256)
        || !valid_digest(&stage.payload_sha256)
    {
        return Err(UpdateError::InvalidStage);
    }
    let bytes = read_private_file(cancellation, &stage.state_path, 4096)?;
    if decode_stage_record(&bytes)? != stage.record() {
        return Err(UpdateError::InvalidStage);
    }
    let package = hash_regular_file(cancellation, &stage.asset_path, stage.size_bytes)?;
    if package != (stage.sha256.clone(), stage.size_bytes) {
        return Err(UpdateError::InvalidStage);
    }
    validate_payload_machine(cancellation, stage)?;
    let payload = hash_regular_file(cancellation, &stage.payload_path, payload_limit)?;
    if payload != (stage.payload_sha256.clone(), stage.payload_size_bytes) {
        return Err(UpdateError::InvalidStage);
    }
    Ok(())
}

/// Loads and fully validates a Go/Rust-compatible durable stage.
///
/// # Errors
/// Rejects unsafe roots or records and every failed revalidation.
pub fn load_stage(
    cancellation: &CancellationToken,
    root: &Path,
) -> Result<StagedUpdate, UpdateError> {
    check_cancel(cancellation)?;
    if !root.is_absolute() {
        return Err(UpdateError::InvalidStage);
    }
    validate_private_path(root, true).map_err(|_| UpdateError::InvalidStage)?;
    let state_path = root.join("state.json");
    validate_private_path(&state_path, false).map_err(|_| UpdateError::InvalidStage)?;
    let record = decode_stage_record(&read_private_file(cancellation, &state_path, 4096)?)?;
    let asset_name = package_name(
        &Target {
            os: record.goos.clone(),
            arch: record.goarch.clone(),
        },
        &record.version,
    )?;
    if asset_name != record.asset_name {
        return Err(UpdateError::InvalidStage);
    }
    let asset_path = root.join(&asset_name);
    let payload_path = match record.kind {
        StageKind::DarwinDmg => asset_path.clone(),
        StageKind::LinuxBinary => root.join("ptrack"),
        StageKind::WindowsZip => root.join("ptrack.exe"),
    };
    let stage = StagedUpdate {
        root: root.to_path_buf(),
        asset_path,
        payload_path,
        state_path,
        version: record.version,
        asset_name: record.asset_name,
        os: record.goos,
        arch: record.goarch,
        sha256: record.sha256,
        size_bytes: record.size_bytes,
        payload_sha256: record.payload_sha256,
        payload_size_bytes: record.payload_size_bytes,
        kind: record.kind,
    };
    validate_stage(cancellation, &stage)?;
    Ok(stage)
}

/// Removes one validated `.stage-*` directory.
///
/// # Errors
/// Refuses broad or non-stage paths and reports removal failures.
pub fn discard_stage(root: &Path) -> Result<(), UpdateError> {
    let name = root
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    if !root.is_absolute() || !name.starts_with(".stage-") || name == ".stage-" {
        return Err(UpdateError::InvalidStage);
    }
    fs::remove_dir_all(root).map_err(|_| UpdateError::InvalidStage)
}

impl StagedUpdate {
    fn record(&self) -> StageRecord {
        StageRecord {
            version: self.version.clone(),
            asset_name: self.asset_name.clone(),
            goos: self.os.clone(),
            goarch: self.arch.clone(),
            sha256: self.sha256.clone(),
            size_bytes: self.size_bytes,
            payload_sha256: self.payload_sha256.clone(),
            payload_size_bytes: self.payload_size_bytes,
            kind: self.kind,
        }
    }
}

pub(crate) fn write_stage_record(stage: &StagedUpdate) -> Result<(), UpdateError> {
    let mut data = serde_json::to_vec(&stage.record()).map_err(|_| UpdateError::InvalidStage)?;
    data.push(b'\n');
    if data.len() > 4096 {
        return Err(UpdateError::InvalidStage);
    }
    let mut file =
        create_private_regular(&stage.state_path).map_err(|_| UpdateError::InvalidStage)?;
    file.write_all(&data)
        .map_err(|_| UpdateError::InvalidStage)?;
    file.sync_all().map_err(|_| UpdateError::InvalidStage)?;
    drop(file);
    secure_private_path(&stage.state_path, false).map_err(|_| UpdateError::InvalidStage)
}

fn decode_stage_record(data: &[u8]) -> Result<StageRecord, UpdateError> {
    let mut deserializer = serde_json::Deserializer::from_slice(data);
    let record =
        StageRecord::deserialize(&mut deserializer).map_err(|_| UpdateError::InvalidStage)?;
    deserializer.end().map_err(|_| UpdateError::InvalidStage)?;
    Ok(record)
}

pub(crate) fn checksum_for(path: &Path, wanted_name: &str) -> Result<String, UpdateError> {
    let file = open_private_regular(path).map_err(|_| UpdateError::InvalidStage)?;
    let mut found = None;
    for (index, line) in BufReader::new(file).split(b'\n').enumerate() {
        if index >= MAX_CHECKSUM_LINES {
            return Err(UpdateError::InvalidStage);
        }
        let mut line = line.map_err(|_| UpdateError::InvalidStage)?;
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        if line.len() > MAX_CHECKSUM_LINE_BYTES || line.len() < 66 || line[64] != b' ' {
            return Err(UpdateError::InvalidStage);
        }
        let digest = std::str::from_utf8(&line[..64]).map_err(|_| UpdateError::InvalidStage)?;
        if !valid_digest(&digest.to_ascii_lowercase()) {
            return Err(UpdateError::InvalidStage);
        }
        let name = std::str::from_utf8(&line[64..])
            .map_err(|_| UpdateError::InvalidStage)?
            .trim();
        if name.is_empty()
            || name.contains(['/', '\\', '\t'])
            || Path::new(name).file_name().and_then(|value| value.to_str()) != Some(name)
        {
            return Err(UpdateError::InvalidStage);
        }
        if name == wanted_name {
            if found.is_some() {
                return Err(UpdateError::InvalidStage);
            }
            found = Some(digest.to_ascii_lowercase());
        }
    }
    found.ok_or(UpdateError::InvalidStage)
}

pub(crate) fn extract_tar_payload(
    cancellation: &CancellationToken,
    asset_path: &Path,
    payload_path: &Path,
) -> Result<(), UpdateError> {
    let file = open_private_regular(asset_path).map_err(|_| UpdateError::InvalidStage)?;
    let gzip = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(gzip.take(MAX_ARCHIVE_TOTAL_BYTES + 1));
    let mut wanted = BTreeMap::from([
        ("LICENSE".to_owned(), false),
        ("README.md".to_owned(), false),
        ("ptrack".to_owned(), false),
    ]);
    let mut total = 0_u64;
    for entry in archive.entries().map_err(|_| UpdateError::InvalidStage)? {
        check_cancel(cancellation)?;
        let mut entry = entry.map_err(|_| UpdateError::InvalidStage)?;
        let path_bytes = entry.path_bytes();
        let raw =
            std::str::from_utf8(path_bytes.as_ref()).map_err(|_| UpdateError::InvalidStage)?;
        let name = raw.strip_prefix("./").unwrap_or(raw);
        if name.is_empty() || name == "." {
            if !entry.header().entry_type().is_dir() {
                return Err(UpdateError::InvalidStage);
            }
            continue;
        }
        let Some(present) = wanted.get_mut(name) else {
            return Err(UpdateError::InvalidStage);
        };
        if *present || name.contains(['/', '\\']) || !entry.header().entry_type().is_file() {
            return Err(UpdateError::InvalidStage);
        }
        let size = entry.size();
        total = total.checked_add(size).ok_or(UpdateError::InvalidStage)?;
        if size > MAX_ARCHIVE_ENTRY_BYTES || total > MAX_ARCHIVE_TOTAL_BYTES {
            return Err(UpdateError::InvalidStage);
        }
        *present = true;
        if name == "ptrack" {
            copy_payload(cancellation, payload_path, &mut entry, size)?;
        } else {
            drain_exact(cancellation, &mut entry, size)?;
        }
    }
    if wanted.values().any(|present| !present) {
        return Err(UpdateError::InvalidStage);
    }
    Ok(())
}

pub(crate) fn extract_zip_payload(
    cancellation: &CancellationToken,
    asset_path: &Path,
    payload_path: &Path,
) -> Result<(), UpdateError> {
    let file = open_private_regular(asset_path).map_err(|_| UpdateError::InvalidStage)?;
    let mut archive = zip::ZipArchive::new(file).map_err(|_| UpdateError::InvalidStage)?;
    let mut wanted = BTreeMap::from([
        ("LICENSE".to_owned(), false),
        ("README.md".to_owned(), false),
        ("ptrack.exe".to_owned(), false),
    ]);
    let mut total = 0_u64;
    for index in 0..archive.len() {
        check_cancel(cancellation)?;
        let mut entry = archive
            .by_index(index)
            .map_err(|_| UpdateError::InvalidStage)?;
        let raw = entry.name();
        let name = raw.strip_prefix("./").unwrap_or(raw);
        let Some(present) = wanted.get_mut(name) else {
            return Err(UpdateError::InvalidStage);
        };
        if *present || name.contains(['/', '\\']) || entry.is_dir() || entry.encrypted() {
            return Err(UpdateError::InvalidStage);
        }
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170_000 != 0 && mode & 0o170_000 != 0o100_000)
        {
            return Err(UpdateError::InvalidStage);
        }
        let size = entry.size();
        total = total.checked_add(size).ok_or(UpdateError::InvalidStage)?;
        if size > MAX_ARCHIVE_ENTRY_BYTES || total > MAX_ARCHIVE_TOTAL_BYTES {
            return Err(UpdateError::InvalidStage);
        }
        *present = true;
        if name == "ptrack.exe" {
            copy_payload(cancellation, payload_path, &mut entry, size)?;
        } else {
            drain_exact(cancellation, &mut entry, size)?;
        }
    }
    if wanted.values().any(|present| !present) {
        return Err(UpdateError::InvalidStage);
    }
    Ok(())
}

fn copy_payload(
    cancellation: &CancellationToken,
    path: &Path,
    reader: &mut dyn Read,
    size: u64,
) -> Result<(), UpdateError> {
    let mut file = create_private_regular(path).map_err(|_| UpdateError::InvalidStage)?;
    let result = copy_exact(cancellation, reader, &mut file, size);
    if result.is_ok() {
        file.sync_all().map_err(|_| UpdateError::InvalidStage)?;
    }
    drop(file);
    if result.is_err() {
        let _ = fs::remove_file(path);
        return result;
    }
    secure_private_path(path, false).map_err(|_| UpdateError::InvalidStage)
}

fn drain_exact(
    cancellation: &CancellationToken,
    reader: &mut dyn Read,
    size: u64,
) -> Result<(), UpdateError> {
    copy_exact(cancellation, reader, &mut io::sink(), size)
}

fn copy_exact(
    cancellation: &CancellationToken,
    reader: &mut dyn Read,
    writer: &mut dyn Write,
    size: u64,
) -> Result<(), UpdateError> {
    let mut remaining = size;
    let mut buffer = vec![0_u8; 64 * 1024].into_boxed_slice();
    while remaining > 0 {
        check_cancel(cancellation)?;
        let maximum = usize::try_from(remaining.min(buffer.len() as u64))
            .map_err(|_| UpdateError::InvalidStage)?;
        let count = reader
            .read(&mut buffer[..maximum])
            .map_err(|_| UpdateError::InvalidStage)?;
        if count == 0 {
            return Err(UpdateError::InvalidStage);
        }
        writer
            .write_all(&buffer[..count])
            .map_err(|_| UpdateError::InvalidStage)?;
        remaining -= u64::try_from(count).map_err(|_| UpdateError::InvalidStage)?;
    }
    Ok(())
}

fn validate_payload_machine(
    cancellation: &CancellationToken,
    stage: &StagedUpdate,
) -> Result<(), UpdateError> {
    check_cancel(cancellation)?;
    match stage.kind {
        StageKind::DarwinDmg => {
            if stage.os != "darwin" || stage.payload_path != stage.asset_path {
                return Err(UpdateError::InvalidStage);
            }
        }
        StageKind::LinuxBinary => {
            if stage.os != "linux" {
                return Err(UpdateError::InvalidStage);
            }
            validate_elf(&stage.payload_path, &stage.arch)?;
        }
        StageKind::WindowsZip => {
            if stage.os != "windows" {
                return Err(UpdateError::InvalidStage);
            }
            validate_pe(&stage.payload_path, &stage.arch)?;
        }
    }
    Ok(())
}

fn validate_elf(path: &Path, arch: &str) -> Result<(), UpdateError> {
    let mut file = open_private_regular(path).map_err(|_| UpdateError::InvalidStage)?;
    let mut header = [0_u8; 64];
    file.read_exact(&mut header)
        .map_err(|_| UpdateError::InvalidStage)?;
    let machine = u16::from_le_bytes([header[18], header[19]]);
    let kind = u16::from_le_bytes([header[16], header[17]]);
    let version = u32::from_le_bytes(header[20..24].try_into().unwrap_or_default());
    let header_size = u16::from_le_bytes([header[52], header[53]]);
    let expected = match arch {
        "amd64" => 62,
        "arm64" => 183,
        _ => return Err(UpdateError::InvalidStage),
    };
    if &header[..4] != b"\x7fELF"
        || header[4] != 2
        || header[5] != 1
        || header[6] != 1
        || !matches!(kind, 2 | 3)
        || machine != expected
        || version != 1
        || header_size < 64
    {
        return Err(UpdateError::InvalidStage);
    }
    Ok(())
}

fn validate_pe(path: &Path, arch: &str) -> Result<(), UpdateError> {
    let mut file = open_private_regular(path).map_err(|_| UpdateError::InvalidStage)?;
    let mut dos = [0_u8; 64];
    file.read_exact(&mut dos)
        .map_err(|_| UpdateError::InvalidStage)?;
    if &dos[..2] != b"MZ" {
        return Err(UpdateError::InvalidStage);
    }
    let offset = u64::from(u32::from_le_bytes(
        dos[0x3c..0x40].try_into().unwrap_or_default(),
    ));
    let file_size = file
        .metadata()
        .map_err(|_| UpdateError::InvalidStage)?
        .len();
    if offset < dos.len() as u64 || offset > file_size.saturating_sub(24) {
        return Err(UpdateError::InvalidStage);
    }
    file.seek(io::SeekFrom::Start(offset))
        .map_err(|_| UpdateError::InvalidStage)?;
    let mut coff = [0_u8; 24];
    file.read_exact(&mut coff)
        .map_err(|_| UpdateError::InvalidStage)?;
    let machine = u16::from_le_bytes([coff[4], coff[5]]);
    let optional_size = u16::from_le_bytes([coff[20], coff[21]]);
    let characteristics = u16::from_le_bytes([coff[22], coff[23]]);
    let mut optional_magic = [0_u8; 2];
    file.read_exact(&mut optional_magic)
        .map_err(|_| UpdateError::InvalidStage)?;
    let expected = match arch {
        "amd64" => 0x8664,
        "arm64" => 0xaa64,
        _ => return Err(UpdateError::InvalidStage),
    };
    if &coff[..4] != b"PE\0\0"
        || machine != expected
        || optional_size < 2
        || u16::from_le_bytes(optional_magic) != 0x20b
        || characteristics & 0x0002 == 0
    {
        return Err(UpdateError::InvalidStage);
    }
    Ok(())
}

pub(crate) fn hash_regular_file(
    cancellation: &CancellationToken,
    path: &Path,
    limit: u64,
) -> Result<(String, u64), UpdateError> {
    let mut file = open_private_regular(path).map_err(|_| UpdateError::InvalidStage)?;
    let mut hash = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024].into_boxed_slice();
    loop {
        check_cancel(cancellation)?;
        let count = file
            .read(&mut buffer)
            .map_err(|_| UpdateError::InvalidStage)?;
        if count == 0 {
            break;
        }
        size = size
            .checked_add(u64::try_from(count).map_err(|_| UpdateError::InvalidStage)?)
            .filter(|value| *value <= limit)
            .ok_or(UpdateError::InvalidStage)?;
        hash.update(&buffer[..count]);
    }
    Ok((hex_lower(&hash.finalize()), size))
}

fn read_private_file(
    cancellation: &CancellationToken,
    path: &Path,
    limit: u64,
) -> Result<Vec<u8>, UpdateError> {
    let mut file = open_private_regular(path).map_err(|_| UpdateError::InvalidStage)?;
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        check_cancel(cancellation)?;
        let count = file
            .read(&mut buffer)
            .map_err(|_| UpdateError::InvalidStage)?;
        if count == 0 {
            break;
        }
        if count
            > usize::try_from(limit)
                .unwrap_or_default()
                .saturating_sub(bytes.len())
        {
            return Err(UpdateError::InvalidStage);
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    Ok(bytes)
}

pub(crate) fn validate_download_url(
    candidate: &reqwest::Url,
    initial: &reqwest::Url,
) -> Result<(), UpdateError> {
    if candidate.scheme() != "https"
        || !candidate.username().is_empty()
        || candidate.password().is_some()
        || candidate.fragment().is_some()
    {
        return Err(UpdateError::InvalidStage);
    }
    if candidate.host_str() == Some("github.com") {
        return (candidate == initial)
            .then_some(())
            .ok_or(UpdateError::InvalidStage);
    }
    if candidate.port().is_some()
        || !matches!(
            candidate.host_str(),
            Some("release-assets.githubusercontent.com" | "objects.githubusercontent.com")
        )
        || candidate.path().is_empty()
        || candidate.path() == "/"
    {
        return Err(UpdateError::InvalidStage);
    }
    Ok(())
}

fn expected_payload(stage: &StagedUpdate) -> PathBuf {
    match stage.kind {
        StageKind::DarwinDmg => stage.asset_path.clone(),
        StageKind::LinuxBinary => stage.root.join("ptrack"),
        StageKind::WindowsZip => stage.root.join("ptrack.exe"),
    }
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn path_within(root: &Path, path: &Path) -> bool {
    path.is_absolute()
        && path.strip_prefix(root).ok().is_some_and(|relative| {
            relative.components().next().is_some()
                && relative
                    .components()
                    .all(|component| matches!(component, Component::Normal(_)))
        })
}

fn check_cancel(cancellation: &CancellationToken) -> Result<(), UpdateError> {
    if cancellation.is_cancelled() {
        Err(UpdateError::Cancelled)
    } else {
        Ok(())
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}
