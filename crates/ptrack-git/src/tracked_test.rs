use std::sync::Arc;

use crate::runner::{CancellationToken, RepositoryError};
use crate::snapshot::RepositoryService;
use crate::test_support::FakeRunner;
use crate::tracked::MAX_TRACKED_PATHS;

const NOW: i64 = 1_785_067_200; // 2026-07-26T12:00:00Z

fn now() -> i64 {
    NOW
}

fn service(output: Result<Vec<u8>, RepositoryError>) -> RepositoryService {
    let runner = Arc::new(FakeRunner::default());
    match output {
        Ok(value) => runner.output("/repo|ls-files", value),
        Err(error) => runner.error("/repo|ls-files", error),
    }
    RepositoryService::with_runner_and_clock(runner, now)
}

#[test]
fn tracked_paths_are_sorted_and_complete() {
    let listing = service(Ok(
        b"src/lib.rs\0Cargo.toml\0frontend/package.json\0".to_vec()
    ))
    .capture_tracked_paths(&CancellationToken::new(), std::path::Path::new("/repo"))
    .expect("capture");
    assert_eq!(
        listing.paths,
        vec![
            "Cargo.toml".to_owned(),
            "frontend/package.json".to_owned(),
            "src/lib.rs".to_owned(),
        ]
    );
    assert!(!listing.incomplete);
}

#[test]
fn a_listing_over_the_cap_is_truncated_and_marked_incomplete() {
    let mut output = Vec::new();
    for index in 0..MAX_TRACKED_PATHS + 5 {
        output.extend_from_slice(format!("src/file{index:07}.rs\0").as_bytes());
    }
    let listing = service(Ok(output))
        .capture_tracked_paths(&CancellationToken::new(), std::path::Path::new("/repo"))
        .expect("capture");
    assert!(listing.incomplete);
    assert_eq!(listing.paths.len(), MAX_TRACKED_PATHS);
}

#[test]
fn an_empty_listing_yields_no_paths() {
    let listing = service(Ok(Vec::new()))
        .capture_tracked_paths(&CancellationToken::new(), std::path::Path::new("/repo"))
        .expect("capture");
    assert!(listing.paths.is_empty());
    assert!(!listing.incomplete);
}

#[test]
fn a_non_utf8_path_is_rejected_without_leaking_it() {
    let error = service(Ok(b"Cargo.toml\0src/\xff\xfe.rs\0".to_vec()))
        .capture_tracked_paths(&CancellationToken::new(), std::path::Path::new("/repo"))
        .expect_err("invalid data");
    assert_eq!(
        error,
        RepositoryError::InvalidData("tracked path is not UTF-8")
    );
}

#[test]
fn a_failed_listing_propagates_its_error() {
    let error = service(Err(RepositoryError::CommandFailed))
        .capture_tracked_paths(&CancellationToken::new(), std::path::Path::new("/repo"))
        .expect_err("command failed");
    assert_eq!(error, RepositoryError::CommandFailed);
}

#[test]
fn cancellation_stops_the_scan() {
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let error = service(Ok(b"Cargo.toml\0".to_vec()))
        .capture_tracked_paths(&cancellation, std::path::Path::new("/repo"))
        .expect_err("cancelled");
    assert_eq!(error, RepositoryError::Cancelled);
}
