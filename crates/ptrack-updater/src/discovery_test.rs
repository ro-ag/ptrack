use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use super::discovery::{
    Client, Target, UpdateError, compare_versions, package_name, parse_version, select_candidate,
};

#[test]
fn strict_versions_and_exact_target_names_are_frozen() {
    assert_eq!(parse_version("v1.2.3", true).unwrap().to_string(), "1.2.3");
    for value in ["1.2", "1.2.3.4", "1.02.3", "1.2.3-beta", " 1.2.3", ""] {
        assert!(parse_version(value, true).is_err(), "accepted {value:?}");
    }
    assert!(compare_versions("1.2.3", "1.3.0").unwrap().is_lt());
    assert_eq!(
        package_name(
            &Target {
                os: "windows".to_owned(),
                arch: "arm64".to_owned(),
            },
            "1.2.3",
        )
        .unwrap(),
        "ptrack_1.2.3_windows_arm64.zip"
    );
    assert!(
        package_name(
            &Target {
                os: "freebsd".to_owned(),
                arch: "amd64".to_owned(),
            },
            "1.2.3"
        )
        .is_err()
    );
}

#[test]
fn release_notes_accept_exact_thirty_two_kibibyte_boundary_only() {
    let target = Target {
        os: "linux".to_owned(),
        arch: "amd64".to_owned(),
    };
    let current = parse_version("1.2.3", true).unwrap();
    assert!(select_candidate(&release_with_notes(32 << 10, &target), current, &target).is_ok());
    assert_eq!(
        select_candidate(
            &release_with_notes((32 << 10) + 1, &target),
            current,
            &target,
        )
        .unwrap_err(),
        UpdateError::InvalidRelease
    );
}

#[test]
fn release_publication_rejects_go_zero_time_but_accepts_unix_epoch() {
    let target = Target {
        os: "linux".to_owned(),
        arch: "amd64".to_owned(),
    };
    let current = parse_version("1.2.3", true).unwrap();
    assert_eq!(
        select_candidate(
            &release_with_publication("0001-01-01T00:00:00Z", &target),
            current,
            &target,
        )
        .unwrap_err(),
        UpdateError::InvalidRelease
    );
    assert!(
        select_candidate(
            &release_with_publication("1970-01-01T00:00:00Z", &target),
            current,
            &target,
        )
        .is_ok()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn production_check_executes_fixed_headers_and_selects_exact_assets() {
    let target = Target {
        os: "linux".to_owned(),
        arch: "amd64".to_owned(),
    };
    let package = package_name(&target, "1.2.4").unwrap();
    let release = serde_json::json!({
        "tag_name": "v1.2.4",
        "body": "notes",
        "draft": false,
        "prerelease": false,
        "published_at": "2026-01-02T03:04:05Z",
        "assets": [
            {"name": package, "browser_download_url": format!("https://github.com/ro-ag/ptrack/releases/download/v1.2.4/{package}"), "size": 123, "state": "uploaded"},
            {"name": "checksums.txt", "browser_download_url": "https://github.com/ro-ag/ptrack/releases/download/v1.2.4/checksums.txt", "size": 80, "state": "uploaded"}
        ]
    })
    .to_string();
    let (endpoint, request) = serve_once(release.into_bytes()).await;
    let candidate = Client::with_endpoint(endpoint)
        .unwrap()
        .check(&CancellationToken::new(), "1.2.3", &target)
        .await
        .unwrap();
    assert_eq!(candidate.version, "1.2.4");
    assert_eq!(candidate.package.name, package);
    let request = request.await.unwrap().to_ascii_lowercase();
    assert!(request.contains("accept: application/vnd.github+json\r\n"));
    assert!(request.contains("user-agent: p-track-updater\r\n"));
    assert!(request.contains("x-github-api-version: 2022-11-28\r\n"));
}

#[tokio::test(flavor = "current_thread")]
async fn discovery_rejects_metadata_over_one_mebibyte() {
    let (endpoint, _) = serve_once(vec![b' '; (1 << 20) + 1]).await;
    let error = Client::with_endpoint(endpoint)
        .unwrap()
        .check(
            &CancellationToken::new(),
            "1.2.3",
            &Target {
                os: "linux".to_owned(),
                arch: "amd64".to_owned(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error, UpdateError::InvalidRelease);
}

async fn serve_once(body: Vec<u8>) -> (String, tokio::task::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let body = Arc::new(body);
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut chunk = [0_u8; 1024];
        loop {
            let count = stream.read(&mut chunk).await.unwrap();
            request.extend_from_slice(&chunk[..count]);
            if count == 0 || request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(header.as_bytes()).await.unwrap();
        stream.write_all(&body).await.unwrap();
        String::from_utf8(request).unwrap()
    });
    (format!("http://{address}/latest"), task)
}

fn release_with_notes(length: usize, target: &Target) -> Vec<u8> {
    release_fixture(&"x".repeat(length), "2026-01-02T03:04:05Z", target)
}

fn release_with_publication(publication: &str, target: &Target) -> Vec<u8> {
    release_fixture("", publication, target)
}

fn release_fixture(body: &str, publication: &str, target: &Target) -> Vec<u8> {
    let package = package_name(target, "1.2.4").unwrap();
    serde_json::to_vec(&serde_json::json!({
        "tag_name": "v1.2.4",
        "body": body,
        "draft": false,
        "prerelease": false,
        "published_at": publication,
        "assets": [
            {"name": package, "browser_download_url": format!("https://github.com/ro-ag/ptrack/releases/download/v1.2.4/{package}"), "size": 123, "state": "uploaded"},
            {"name": "checksums.txt", "browser_download_url": "https://github.com/ro-ag/ptrack/releases/download/v1.2.4/checksums.txt", "size": 80, "state": "uploaded"}
        ]
    }))
    .unwrap()
}
