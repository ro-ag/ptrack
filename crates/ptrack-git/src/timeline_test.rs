use std::path::Path;

use crate::runner::{CancellationToken, RepositoryError};
use crate::test_support::FakeRunner;
use crate::timeline::{MAX_TIMELINE_COMMITS, TimelineTag, capture_timeline_with};

fn root() -> &'static Path {
    Path::new("/repo")
}

fn tag_line(at: i64, name: &str) -> String {
    format!("{at}\u{1f}{name}\n")
}

#[test]
fn history_is_returned_oldest_first_with_tags_sorted_by_creation() {
    let runner = FakeRunner::default();
    // git log reports newest first.
    runner.output("/repo|log", "300\n200\n100\n");
    runner.output(
        "/repo|tag",
        format!("{}{}", tag_line(250, "v0.2.0"), tag_line(150, "v0.1.0")),
    );

    let timeline = capture_timeline_with(&runner, &CancellationToken::default(), root())
        .expect("timeline reads");

    assert_eq!(
        timeline.commits,
        vec![100, 200, 300],
        "a timeline reads forwards even though git logs backwards",
    );
    assert_eq!(
        timeline.tags,
        vec![
            TimelineTag {
                name: "v0.1.0".to_owned(),
                at: 150
            },
            TimelineTag {
                name: "v0.2.0".to_owned(),
                at: 250
            },
        ],
    );
    assert!(!timeline.truncated);
}

#[test]
fn a_repository_without_tags_still_yields_its_commits() {
    let runner = FakeRunner::default();
    runner.output("/repo|log", "10\n");
    runner.output("/repo|tag", "");

    let timeline = capture_timeline_with(&runner, &CancellationToken::default(), root())
        .expect("timeline reads");

    assert_eq!(timeline.commits, vec![10]);
    assert!(timeline.tags.is_empty());
}

#[test]
fn a_failing_tag_read_does_not_cost_the_commit_history() {
    let runner = FakeRunner::default();
    // Newest first, the way git log reports it.
    runner.output("/repo|log", "20\n10\n");
    runner.error("/repo|tag", RepositoryError::CommandFailed);

    let timeline = capture_timeline_with(&runner, &CancellationToken::default(), root())
        .expect("commits survive a tag failure");

    assert_eq!(timeline.commits, vec![10, 20]);
    assert!(timeline.tags.is_empty());
}

#[test]
fn an_aggregate_limit_on_the_tag_read_fails_the_whole_capture() {
    let runner = FakeRunner::default();
    runner.output("/repo|log", "10\n");
    runner.error("/repo|tag", RepositoryError::AggregateLimit);

    // A resource bound is the one tag failure that must not be swallowed: it
    // says the session has already read too much, not that tags are missing.
    assert_eq!(
        capture_timeline_with(&runner, &CancellationToken::default(), root()),
        Err(RepositoryError::AggregateLimit),
    );
}

#[test]
fn history_longer_than_the_cap_is_truncated_and_says_so() {
    let runner = FakeRunner::default();
    let mut log = String::new();
    for index in (0..=MAX_TIMELINE_COMMITS).rev() {
        use std::fmt::Write as _;
        writeln!(log, "{index}").expect("string write");
    }
    runner.output("/repo|log", log);
    runner.output("/repo|tag", "");

    let timeline = capture_timeline_with(&runner, &CancellationToken::default(), root())
        .expect("timeline reads");

    assert_eq!(timeline.commits.len(), MAX_TIMELINE_COMMITS);
    assert!(
        timeline.truncated,
        "the interface has to be able to say the history runs back further",
    );
    // The newest commits are the ones kept, and they end the series.
    assert_eq!(
        timeline.commits.last().copied(),
        Some(i64::try_from(MAX_TIMELINE_COMMITS).expect("cap fits")),
    );
    assert_eq!(
        timeline.commits.first().copied(),
        Some(1),
        "the oldest commit is the one dropped by the cap",
    );
}

#[test]
fn malformed_records_are_skipped_rather_than_failing_the_read() {
    let runner = FakeRunner::default();
    runner.output("/repo|log", "30\nnot-a-number\n\n10\n");
    runner.output(
        "/repo|tag",
        format!("{}{}{}", tag_line(40, "v1"), "garbage\n", "50\u{1f}\n"),
    );

    let timeline = capture_timeline_with(&runner, &CancellationToken::default(), root())
        .expect("timeline reads");

    assert_eq!(timeline.commits, vec![10, 30]);
    assert_eq!(
        timeline.tags,
        vec![TimelineTag {
            name: "v1".to_owned(),
            at: 40
        }],
        "a tag with no name is not a tag",
    );
}

#[test]
fn cancellation_is_reported_rather_than_returning_an_empty_history() {
    let runner = FakeRunner::default();
    let cancellation = CancellationToken::default();
    cancellation.cancel();

    assert_eq!(
        capture_timeline_with(&runner, &cancellation, root()),
        Err(RepositoryError::Cancelled),
    );
}

#[test]
fn the_commit_read_asks_git_for_timestamps_only() {
    let runner = FakeRunner::default();
    runner.output("/repo|log", "10\n");
    runner.output("/repo|tag", "");

    capture_timeline_with(&runner, &CancellationToken::default(), root()).expect("timeline reads");

    let calls = runner.calls();
    let log = calls
        .iter()
        .find(|(_, args)| args.first().is_some_and(|arg| arg == "log"))
        .expect("a log call");
    assert!(
        log.1.iter().any(|arg| arg == "--format=%at"),
        "only timestamps cross the boundary: {:?}",
        log.1,
    );
    assert!(
        !log.1.iter().any(|arg| arg == "--name-only"),
        "the timeline never needs file names",
    );
}
