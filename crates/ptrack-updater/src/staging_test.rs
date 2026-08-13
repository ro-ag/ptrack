use std::fs::{self, File};
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::write::GzEncoder;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use zip::write::SimpleFileOptions;

use super::staging::{
    StageKind, StagedUpdate, checksum_for, extract_tar_payload, extract_zip_payload,
    hash_regular_file, load_stage, validate_download_url, validate_stage, write_stage_record,
};
use super::{Asset, Candidate, Client, Target};

#[tokio::test(flavor = "current_thread")]
async fn real_http_stage_streams_both_assets_and_publishes_only_after_verification() {
    let base = private_temp_dir();
    let source = base.join("source.tar.gz");
    make_tar(&source, &fake_elf(62));
    let package = fs::read(&source).unwrap();
    fs::remove_file(source).unwrap();
    let digest = hex_lower(&Sha256::digest(&package));
    let asset_name = "ptrack_1.2.4_linux_amd64.tar.gz";
    let manifest = format!("{digest}  {asset_name}\n").into_bytes();
    let server = AssetServer::start(manifest.clone(), package.clone()).await;
    let candidate = Candidate {
        version: "1.2.4".to_owned(),
        tag: "v1.2.4".to_owned(),
        page_url: "https://github.com/ro-ag/ptrack/releases/tag/v1.2.4".to_owned(),
        published_at: "2026-08-13T00:00:00Z".to_owned(),
        notes: String::new(),
        package: Asset {
            name: asset_name.to_owned(),
            download_url: format!(
                "https://github.com/ro-ag/ptrack/releases/download/v1.2.4/{asset_name}"
            ),
            size_bytes: package.len() as u64,
        },
        checksums: Asset {
            name: "checksums.txt".to_owned(),
            download_url: "https://github.com/ro-ag/ptrack/releases/download/v1.2.4/checksums.txt"
                .to_owned(),
            size_bytes: manifest.len() as u64,
        },
    };
    let progress = MutexProgress::default();
    let stage = Client::with_test_asset_server(server.base.clone())
        .unwrap()
        .stage(
            &CancellationToken::new(),
            &candidate,
            &Target {
                os: "linux".to_owned(),
                arch: "amd64".to_owned(),
            },
            &base,
            Some(&|item| progress.push(item)),
        )
        .await
        .unwrap();
    validate_stage(&CancellationToken::new(), &stage).unwrap();
    assert_eq!(stage.sha256, digest);
    assert_eq!(progress.last().unwrap().downloaded, package.len() as u64);
    server.finish().await;
    cleanup(&base);
}

#[test]
fn checksum_manifest_has_exact_bounded_compatibility_grammar() {
    let root = private_temp_dir();
    let package = "ptrack_1.2.3_linux_amd64.tar.gz";
    let digest = "ab".repeat(32);
    let manifest = root.join("checksums.txt");
    write_private(&manifest, format!("{digest}  {package}\n").as_bytes());
    assert_eq!(checksum_for(&manifest, package).unwrap(), digest);

    write_private_replace(&manifest, format!("{digest}  ../{package}\n").as_bytes());
    assert!(checksum_for(&manifest, package).is_err());
    cleanup(&root);
}

#[test]
fn checksum_manifest_preserves_effective_line_and_entry_boundaries() {
    use std::fmt::Write as _;

    let root = private_temp_dir();
    let digest = "cd".repeat(32);
    let manifest = root.join("checksums.txt");
    let wanted = "p".repeat(900);
    let mut lines = (0..255).fold(String::new(), |mut output, index| {
        use std::fmt::Write as _;
        let _ = writeln!(output, "{digest}  other-{index}");
        output
    });
    let _ = writeln!(lines, "{digest}  {wanted}");
    write_private(&manifest, lines.as_bytes());
    assert_eq!(checksum_for(&manifest, &wanted).unwrap(), digest);

    let _ = writeln!(lines, "{}  overflow", "ef".repeat(32));
    write_private_replace(&manifest, lines.as_bytes());
    assert!(checksum_for(&manifest, &wanted).is_err());

    let too_long = "q".repeat(960);
    write_private_replace(
        &manifest,
        format!("{}  {too_long}\n", "ef".repeat(32)).as_bytes(),
    );
    assert!(checksum_for(&manifest, &too_long).is_err());
    cleanup(&root);
}

#[test]
fn asset_redirects_are_exact_https_github_hosts_without_embedded_authority() {
    let initial = reqwest::Url::parse(
        "https://github.com/ro-ag/ptrack/releases/download/v1.2.3/checksums.txt",
    )
    .unwrap();
    assert!(validate_download_url(&initial, &initial).is_ok());
    for accepted in [
        "https://release-assets.githubusercontent.com/a/b?x=1",
        "https://objects.githubusercontent.com/a/b",
    ] {
        assert!(validate_download_url(&reqwest::Url::parse(accepted).unwrap(), &initial).is_ok());
    }
    for rejected in [
        "http://release-assets.githubusercontent.com/a",
        "https://user@release-assets.githubusercontent.com/a",
        "https://release-assets.githubusercontent.com:444/a",
        "https://release-assets.githubusercontent.com/",
        "https://release-assets.githubusercontent.com/a#fragment",
        "https://evil.example/a",
        "https://github.com/other",
    ] {
        assert!(
            validate_download_url(&reqwest::Url::parse(rejected).unwrap(), &initial).is_err(),
            "accepted {rejected}"
        );
    }
}

#[test]
fn linux_archive_stage_round_trips_and_rejects_payload_tampering() {
    let cancellation = CancellationToken::new();
    let root = private_temp_dir();
    let version = "1.2.3";
    let asset_name = format!("ptrack_{version}_linux_amd64.tar.gz");
    let asset_path = root.join(&asset_name);
    make_tar(&asset_path, &fake_elf(62));
    let payload_path = root.join("ptrack");
    extract_tar_payload(&cancellation, &asset_path, &payload_path).unwrap();
    let (sha256, size_bytes) = hash_regular_file(&cancellation, &asset_path, 512 << 20).unwrap();
    let (payload_sha256, payload_size_bytes) =
        hash_regular_file(&cancellation, &payload_path, 128 << 20).unwrap();
    let stage = StagedUpdate {
        root: root.clone(),
        asset_path,
        payload_path: payload_path.clone(),
        state_path: root.join("state.json"),
        version: version.to_owned(),
        asset_name,
        os: "linux".to_owned(),
        arch: "amd64".to_owned(),
        sha256,
        size_bytes,
        payload_sha256,
        payload_size_bytes,
        kind: StageKind::LinuxBinary,
    };
    write_stage_record(&stage).unwrap();
    validate_stage(&cancellation, &stage).unwrap();
    assert_eq!(load_stage(&cancellation, &root).unwrap(), stage);

    write_private_replace(&payload_path, &fake_elf(62));
    File::options()
        .append(true)
        .open(&payload_path)
        .unwrap()
        .write_all(b"tamper")
        .unwrap();
    assert!(validate_stage(&cancellation, &stage).is_err());
    cleanup(&root);
}

#[test]
fn windows_zip_accepts_only_exact_root_files_and_machine() {
    let cancellation = CancellationToken::new();
    let root = private_temp_dir();
    let good = root.join("good.zip");
    make_zip(&good, &fake_pe(0xaa64), false);
    extract_zip_payload(&cancellation, &good, &root.join("ptrack.exe")).unwrap();

    let bad = root.join("bad.zip");
    make_zip(&bad, &fake_pe(0xaa64), true);
    assert!(extract_zip_payload(&cancellation, &bad, &root.join("other.exe")).is_err());
    cleanup(&root);
}

#[test]
fn durable_record_is_exact_bounded_json_and_unknown_fields_fail_closed() {
    let cancellation = CancellationToken::new();
    let root = private_temp_dir();
    let name = "p-track_1.2.3_darwin_arm64.dmg";
    let asset = root.join(name);
    write_private(&asset, b"dmg");
    let (digest, size) = hash_regular_file(&cancellation, &asset, 512 << 20).unwrap();
    let stage = StagedUpdate {
        root: root.clone(),
        asset_path: asset.clone(),
        payload_path: asset,
        state_path: root.join("state.json"),
        version: "1.2.3".to_owned(),
        asset_name: name.to_owned(),
        os: "darwin".to_owned(),
        arch: "arm64".to_owned(),
        sha256: digest.clone(),
        size_bytes: size,
        payload_sha256: digest,
        payload_size_bytes: size,
        kind: StageKind::DarwinDmg,
    };
    write_stage_record(&stage).unwrap();
    let exact = fs::read_to_string(&stage.state_path).unwrap();
    assert_eq!(
        exact,
        format!(
            "{{\"version\":\"1.2.3\",\"asset_name\":\"{name}\",\"goos\":\"darwin\",\"goarch\":\"arm64\",\"sha256\":\"{}\",\"size_bytes\":3,\"payload_sha256\":\"{}\",\"payload_size_bytes\":3,\"kind\":\"darwin-dmg\"}}\n",
            stage.sha256, stage.payload_sha256
        )
    );
    let tampered = exact.replacen("\"kind\"", "\"unknown\":true,\"kind\"", 1);
    write_private_replace(&stage.state_path, tampered.as_bytes());
    assert!(load_stage(&cancellation, &root).is_err());
    cleanup(&root);
}

fn make_tar(path: &Path, payload: &[u8]) {
    let file = File::create(path).unwrap();
    let gzip = GzEncoder::new(file, Compression::fast());
    let mut archive = tar::Builder::new(gzip);
    for (name, data) in [
        ("ptrack", payload),
        ("README.md", b"readme"),
        ("LICENSE", b"license"),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        archive.append_data(&mut header, name, data).unwrap();
    }
    archive
        .into_inner()
        .unwrap()
        .finish()
        .unwrap()
        .sync_all()
        .unwrap();
    secure_file(path);
}

fn make_zip(path: &Path, payload: &[u8], extra: bool) {
    let file = File::create(path).unwrap();
    let mut archive = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default().unix_permissions(0o644);
    for (name, data) in [
        ("ptrack.exe", payload),
        ("README.md", b"readme"),
        ("LICENSE", b"license"),
    ] {
        archive.start_file(name, options).unwrap();
        archive.write_all(data).unwrap();
    }
    if extra {
        archive.start_file("extra", options).unwrap();
        archive.write_all(b"no").unwrap();
    }
    archive.finish().unwrap().sync_all().unwrap();
    secure_file(path);
}

fn fake_elf(machine: u16) -> Vec<u8> {
    let mut bytes = vec![0_u8; 64];
    bytes[..4].copy_from_slice(b"\x7fELF");
    bytes[4] = 2;
    bytes[5] = 1;
    bytes[6] = 1;
    bytes[16..18].copy_from_slice(&2_u16.to_le_bytes());
    bytes[18..20].copy_from_slice(&machine.to_le_bytes());
    bytes[20..24].copy_from_slice(&1_u32.to_le_bytes());
    bytes[52..54].copy_from_slice(&64_u16.to_le_bytes());
    bytes
}

fn fake_pe(machine: u16) -> Vec<u8> {
    let mut bytes = vec![0_u8; 90];
    bytes[..2].copy_from_slice(b"MZ");
    bytes[0x3c..0x40].copy_from_slice(&64_u32.to_le_bytes());
    bytes[64..68].copy_from_slice(b"PE\0\0");
    bytes[68..70].copy_from_slice(&machine.to_le_bytes());
    bytes[84..86].copy_from_slice(&2_u16.to_le_bytes());
    bytes[86..88].copy_from_slice(&2_u16.to_le_bytes());
    bytes[88..90].copy_from_slice(&0x20b_u16.to_le_bytes());
    bytes
}

fn private_temp_dir() -> PathBuf {
    let mut random = [0_u8; 16];
    getrandom::fill(&mut random).unwrap();
    let suffix = random.iter().fold(String::new(), |mut output, byte| {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
        output
    });
    let root = std::env::temp_dir().join(format!("ptrack-updater-test-{suffix}"));
    fs::create_dir(&root).unwrap();
    #[cfg(unix)]
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    root
}

fn write_private(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
    secure_file(path);
}

fn write_private_replace(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
    secure_file(path);
}

fn secure_file(path: &Path) {
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    #[cfg(not(unix))]
    let _ = path;
}

fn cleanup(path: &Path) {
    fs::remove_dir_all(path).unwrap();
}

#[derive(Default)]
struct MutexProgress(std::sync::Mutex<Vec<super::Progress>>);

impl MutexProgress {
    fn push(&self, progress: super::Progress) {
        self.0.lock().unwrap().push(progress);
    }

    fn last(&self) -> Option<super::Progress> {
        self.0.lock().unwrap().last().cloned()
    }
}

struct AssetServer {
    base: String,
    task: tokio::task::JoinHandle<Vec<String>>,
}

impl AssetServer {
    async fn start(manifest: Vec<u8>, package: Vec<u8>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let mut paths = Vec::new();
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buffer = [0_u8; 1024];
                loop {
                    let count = stream.read(&mut buffer).await.unwrap();
                    request.extend_from_slice(&buffer[..count]);
                    if count == 0 || request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let line = String::from_utf8(request).unwrap();
                let path = line
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap()
                    .to_owned();
                let body = if path == "/checksums" {
                    &manifest
                } else {
                    &package
                };
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(header.as_bytes()).await.unwrap();
                stream.write_all(body).await.unwrap();
                paths.push(path);
            }
            paths
        });
        Self {
            base: format!("http://{address}"),
            task,
        }
    }

    async fn finish(self) {
        assert_eq!(self.task.await.unwrap(), ["/checksums", "/package"]);
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut output, byte| {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
        output
    })
}
