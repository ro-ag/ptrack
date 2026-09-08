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
    service_with_counts(output, Err(RepositoryError::CommandFailed))
}

fn service_with_counts(
    listing: Result<Vec<u8>, RepositoryError>,
    counts: Result<Vec<u8>, RepositoryError>,
) -> RepositoryService {
    let runner = Arc::new(FakeRunner::default());
    match listing {
        Ok(value) => runner.output("/repo|ls-files", value),
        Err(error) => runner.error("/repo|ls-files", error),
    }
    match counts {
        Ok(value) => runner.output("/repo|grep", value),
        Err(error) => runner.error("/repo|grep", error),
    }
    RepositoryService::with_runner_and_clock(runner, now)
}

fn path_names(listing: &crate::tracked::TrackedPaths) -> Vec<&str> {
    listing
        .paths
        .iter()
        .map(|entry| entry.path.as_str())
        .collect()
}

#[test]
fn tracked_paths_are_sorted_and_complete() {
    let listing = service(Ok(
        b"src/lib.rs\0Cargo.toml\0frontend/package.json\0".to_vec()
    ))
    .capture_tracked_paths(&CancellationToken::new(), std::path::Path::new("/repo"))
    .expect("capture");
    assert_eq!(
        path_names(&listing),
        vec!["Cargo.toml", "frontend/package.json", "src/lib.rs"]
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

#[test]
fn line_counts_attach_to_their_tracked_paths() {
    let listing = service_with_counts(
        Ok(b"Cargo.toml\0src/lib.rs\0assets/logo.png\0".to_vec()),
        Ok(b"HEAD:Cargo.toml\x0012\nHEAD:src/lib.rs\x00400\n".to_vec()),
    )
    .capture_tracked_paths(&CancellationToken::new(), std::path::Path::new("/repo"))
    .expect("capture");
    assert!(listing.lines_counted);
    let counts: Vec<(&str, u32)> = listing
        .paths
        .iter()
        .map(|entry| (entry.path.as_str(), entry.lines))
        .collect();
    // The PNG never appears in the count output: `git grep -I` skips binaries,
    // and an absent record means no counted lines.
    assert_eq!(
        counts,
        vec![
            ("Cargo.toml", 12),
            ("assets/logo.png", 0),
            ("src/lib.rs", 400)
        ]
    );
}

#[test]
fn a_repository_with_no_countable_head_still_lists_its_files() {
    let listing = service_with_counts(
        Ok(b"Cargo.toml\0".to_vec()),
        Err(RepositoryError::CommandFailed),
    )
    .capture_tracked_paths(&CancellationToken::new(), std::path::Path::new("/repo"))
    .expect("capture");
    assert!(!listing.lines_counted);
    assert_eq!(path_names(&listing), vec!["Cargo.toml"]);
    assert_eq!(listing.paths[0].lines, 0);
}

#[test]
fn an_unparsable_count_record_is_skipped_rather_than_failing_the_scan() {
    let listing = service_with_counts(
        Ok(b"Cargo.toml\0src/lib.rs\0".to_vec()),
        Ok(b"garbage\nHEAD:src/lib.rs\x00400\nHEAD:Cargo.toml\x00nine\n".to_vec()),
    )
    .capture_tracked_paths(&CancellationToken::new(), std::path::Path::new("/repo"))
    .expect("capture");
    assert!(listing.lines_counted);
    assert_eq!(listing.paths[0].lines, 0);
    assert_eq!(listing.paths[1].lines, 400);
}
