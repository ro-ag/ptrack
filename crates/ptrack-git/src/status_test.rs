use crate::runner::RepositoryError;
use crate::status::{parse_porcelain_v2_status, parse_status_records};

#[test]
fn porcelain_v2_status_matches_branch_counts_paths_and_bounds() {
    let input = b"# branch.oid abcdef0123456789\0\
        # branch.head feature/workspace\0\
        # branch.upstream origin/feature/workspace\0\
        # branch.ab +3 -2\0\
        1 M. N... 100644 100644 100644 a b staged.go\0\
        1 .M N... 100644 100644 100644 a b unstaged.go\0\
        2 MM N... 100644 100644 100644 a b R100 renamed.go\0old.go\0\
        u UU N... 100644 100644 100644 100644 a b c conflict.go\0\
        ? new.go\0! generated.bin\0";
    let status = parse_porcelain_v2_status(input).expect("valid porcelain status");
    assert_eq!(status.oid, "abcdef0123456789");
    assert_eq!(status.branch, "feature/workspace");
    assert_eq!(status.upstream, "origin/feature/workspace");
    assert_eq!((status.ahead, status.behind), (3, 2));
    assert_eq!(
        (
            status.staged,
            status.unstaged,
            status.untracked,
            status.conflicted,
            status.ignored
        ),
        (2, 2, 1, 1, 1)
    );
    assert_eq!(
        status
            .changed_paths
            .as_ref()
            .expect("changed paths present")
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        [
            "conflict.go",
            "old.go",
            "renamed.go",
            "staged.go",
            "unstaged.go"
        ]
    );
    assert_eq!(
        status
            .untracked_paths
            .as_ref()
            .expect("untracked paths present"),
        &["new.go".to_owned()]
    );
    assert_eq!(status.changed_path_bounds.shown, 5);
    assert_eq!(status.untracked_path_bounds.total, 1);
}

#[test]
fn porcelain_v2_status_handles_detached_initial_and_unknown_headers() {
    let detached = parse_porcelain_v2_status(
        b"# branch.oid abc\0# branch.head (detached)\0# future.header value\0",
    )
    .expect("detached status");
    assert!(detached.detached);
    assert!(!detached.initial);

    let initial = parse_porcelain_v2_status(b"# branch.oid (initial)\0# branch.head (initial)\0")
        .expect("initial status");
    assert!(initial.initial);
    assert!(!initial.detached);
}

#[test]
fn porcelain_v2_status_rejects_malformed_and_escaping_paths() {
    for input in [
        b"# branch.ab ahead behind\0".as_slice(),
        b"1 X\0",
        b"2 R. too-short\0",
        b"2 R. N... 1 1 1 a b R100 new\0",
        b"unexpected\0",
        b"? ../secret\0",
        b"? /absolute\0",
        b"? nested/../../secret\0",
    ] {
        assert!(
            matches!(
                parse_porcelain_v2_status(input),
                Err(RepositoryError::InvalidData(_))
            ),
            "accepted malformed input {input:?}"
        );
    }
}

#[test]
fn porcelain_v2_status_deduplicates_sorts_and_caps_paths() {
    let mut input = Vec::new();
    for index in (0..503).rev() {
        input.extend_from_slice(format!("? path-{index:04}\0").as_bytes());
    }
    input.extend_from_slice(b"? path-0000\0");
    let status = parse_porcelain_v2_status(&input).expect("bounded status");
    assert_eq!(status.untracked, 504);
    let paths = status.untracked_paths.expect("parsed paths are present");
    assert_eq!(paths.len(), 500);
    assert_eq!(paths[0], "path-0000");
    assert_eq!(status.untracked_path_bounds.total, 503);
    assert_eq!(status.untracked_path_bounds.more, 3);
}

#[test]
fn porcelain_v2_status_counts_but_does_not_list_unrepresentable_paths() {
    let status = parse_porcelain_v2_status(
        b"? control\npath\0? \xff-latin1\0? plain.txt\0\
          1 .M N... 100644 100644 100644 a b tab\tname\0\
          1 .M N... 100644 100644 100644 a b ok.rs\0",
    )
    .expect("unusual paths must not fail the status");
    assert_eq!((status.untracked, status.unstaged), (3, 2));
    assert_eq!(
        status.untracked_paths.as_deref(),
        Some(&["plain.txt".to_owned()][..])
    );
    assert_eq!(
        (
            status.untracked_path_bounds.shown,
            status.untracked_path_bounds.total,
            status.untracked_path_bounds.more
        ),
        (1, 3, 2)
    );
    assert_eq!(
        status.changed_paths.as_deref(),
        Some(&["ok.rs".to_owned()][..])
    );
    assert_eq!(status.changed_path_bounds.more, 1);
}

#[test]
fn porcelain_v2_status_reports_whether_the_upstream_resolved() {
    let tracked = parse_status_records(
        b"# branch.oid abc\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +0 -0\0",
    )
    .expect("tracked status");
    assert!(tracked.upstream_resolved);

    let gone = parse_status_records(
        b"# branch.oid abc\0# branch.head main\0# branch.upstream origin/main\0",
    )
    .expect("gone upstream status");
    assert_eq!(gone.status.upstream, "origin/main");
    assert!(!gone.upstream_resolved);
}

#[test]
fn porcelain_v2_status_decodes_non_utf8_headers_lossily() {
    let status = parse_porcelain_v2_status(b"# branch.oid abc\0# branch.head caf\xe9\0")
        .expect("non-UTF-8 branch name must not fail the status");
    assert_eq!(status.branch, "caf\u{fffd}");
}
