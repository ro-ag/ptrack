use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::model::{ChangedArea, RepositoryState};
use crate::runner::{CancellationToken, RepositoryError};
use crate::snapshot::{ExecutionSession, RepositoryService};
use crate::test_support::{FakeRunner, canonical, native_path, run_git};

const NOW: i64 = 1_785_067_200; // 2026-07-26T12:00:00Z

fn now() -> i64 {
    NOW
}

#[test]
#[allow(clippy::too_many_lines)]
fn capture_builds_bounded_read_only_snapshot_and_safe_ranges() {
    let old = NOW - 120 * 24 * 60 * 60;
    let recent = NOW - 60 * 60;
    let runner = Arc::new(FakeRunner::default());
    runner.output(
        "/repo|rev-parse",
        b"true\n/repo\n/repo/.git/worktrees/feature\n/repo/.git\nfalse\n".to_vec(),
    );
    runner.error("/repo|worktree", RepositoryError::CommandFailed);
    runner.output(
        "/repo|status",
        b"# branch.oid abc\0# branch.head feature\0\
          # branch.upstream origin/feature\0# branch.ab +2 -1\0\
          1 M. N... 1 1 1 a b staged.go\0? new.go\0"
            .to_vec(),
    );
    runner.output(
        "/repo|config",
        b"remote.origin.url\nhttps://example.test/fetch.git\0\
          remote.origin.pushurl\nssh://example.test/push.git\0\
          remote.backup.url\nhttps://example.test/backup.git\0"
            .to_vec(),
    );
    runner.output(
        "/repo|for-each-ref:refs/heads",
        format!(
            "{}{}",
            ref_record(
                "refs/heads/feature",
                "abc",
                "origin/feature",
                recent,
                "*",
                "/repo"
            ),
            ref_record("refs/heads/old", "def", "", old, " ", "")
        ),
    );
    runner.output(
        "/repo|for-each-ref:refs/remotes",
        ref_record("refs/remotes/origin/feature", "abc", "", recent, " ", ""),
    );
    let logs = format!(
        "{}{}",
        log_record(
            &"a".repeat(40),
            "Ada",
            "ada@example.test",
            recent,
            "Workspace snapshot",
            "HEAD -> feature",
            &["internal/gui/app.go", "frontend/src/app.js"]
        ),
        log_record(
            &"d".repeat(40),
            "Lin",
            "lin@example.test",
            old,
            "Old work",
            "",
            &["README.md"]
        )
    );
    runner.output("/repo|log", logs.clone());
    runner.output("/repo|rev-list", b"1\t2\n".to_vec());
    runner.output("/repo|log:range", logs);

    let service = RepositoryService::with_runner_and_clock(runner.clone(), now);
    let snapshot = service
        .capture(&CancellationToken::new(), "/repo")
        .expect("capture snapshot");
    assert_eq!(snapshot.state, RepositoryState::Ready);
    assert_eq!(snapshot.root, native_path("/repo"));
    assert!(snapshot.linked_worktree);
    assert!(snapshot.worktrees_incomplete);
    assert_eq!(snapshot.status.branch, "feature");
    assert_eq!(
        (
            snapshot.status.staged,
            snapshot.status.untracked,
            snapshot.status.ahead,
            snapshot.status.behind
        ),
        (1, 1, 2, 1)
    );
    let remotes = snapshot.remotes.as_ref().expect("remotes queried");
    assert_eq!(remotes.len(), 2);
    assert_eq!(remotes[0].name, "backup");
    assert_eq!(remotes[1].push_urls, ["ssh://example.test/push.git"]);
    let local_branches = snapshot.local_branches.as_ref().expect("locals queried");
    assert_eq!(local_branches.len(), 2);
    assert!(local_branches[1].stale);
    assert_eq!(local_branches[0].last_commit_at, "2026-07-26T11:00:00Z");
    let recent_commits = snapshot.recent_commits.as_ref().expect("log queried");
    assert_eq!(recent_commits.len(), 2);
    assert_eq!(recent_commits[0].author_name, "Ada");
    assert_eq!(recent_commits[0].files_changed, 2);
    assert_eq!(
        recent_commits[0].changed_areas,
        [
            ChangedArea {
                name: "frontend".to_owned(),
                files: 1
            },
            ChangedArea {
                name: "internal".to_owned(),
                files: 1
            }
        ]
    );
    assert_eq!(
        snapshot.divergence.as_ref().map(|value| value.ahead),
        Some(2)
    );
    assert_eq!(snapshot.unpushed_commits.as_ref().map(Vec::len), Some(2));
    assert_eq!(
        snapshot.stale_branch_policy,
        "non-current local branch tip older than 90 days; not proof that deletion is safe"
    );

    let calls = runner.calls();
    assert_eq!(calls.len(), 10);
    let status_call = calls
        .iter()
        .find(|(_, args)| args.first().is_some_and(|command| command == "status"))
        .expect("status queried");
    assert!(
        status_call.1.iter().all(|arg| arg != "--ignored=matching"),
        "the fatal status read must not list ignored paths"
    );
    let mutating = [
        "add", "branch", "checkout", "clean", "commit", "fetch", "merge", "pull", "push", "rebase",
        "reset", "restore", "switch", "tag",
    ];
    assert!(calls.iter().all(|(_, args)| {
        args.first()
            .is_none_or(|command| !mutating.iter().any(|item| command == item))
    }));
    let range_calls: Vec<_> = calls
        .iter()
        .filter(|(_, args)| {
            matches!(
                args.first().and_then(|arg| arg.to_str()),
                Some("log" | "rev-list")
            ) && args
                .iter()
                .any(|arg| arg.to_string_lossy().contains("origin/feature.."))
        })
        .collect();
    assert_eq!(range_calls.len(), 2);
    assert!(
        range_calls
            .iter()
            .all(|(_, args)| args.iter().any(|arg| arg == "--end-of-options"))
    );
}

#[test]
fn capture_reports_non_repository_and_propagates_typed_failures() {
    let runner = Arc::new(FakeRunner::default());
    runner.error(
        "/definitely/not/a/repository|rev-parse",
        RepositoryError::CommandFailed,
    );
    let snapshot = RepositoryService::with_runner_and_clock(runner, now)
        .capture(&CancellationToken::new(), "/definitely/not/a/repository")
        .expect("non-repository is not an error");
    assert_eq!(snapshot.state, RepositoryState::NotRepository);

    for error in [
        RepositoryError::Cancelled,
        RepositoryError::CommandTimeout,
        RepositoryError::OutputLimit,
    ] {
        let runner = Arc::new(FakeRunner::default());
        runner.error("/repo|rev-parse", error.clone());
        assert_eq!(
            RepositoryService::with_runner_and_clock(runner, now)
                .capture(&CancellationToken::new(), "/repo"),
            Err(error)
        );
    }
}

#[test]
fn capture_aggregate_budget_is_hard() {
    let runner = FakeRunner::default();
    for _ in 0..3 {
        runner.output("/repo|status", vec![b'x'; 4 * 1024 * 1024]);
    }
    runner.output("/repo|status", b"x".to_vec());
    let token = CancellationToken::new();
    let mut session = ExecutionSession::new(&runner, &token);
    let command = [OsString::from("status")];
    for _ in 0..3 {
        assert_eq!(
            session
                .run(Path::new("/repo"), &command)
                .map(|value| value.len()),
            Ok(4 * 1024 * 1024)
        );
    }
    assert_eq!(
        session.run(Path::new("/repo"), &command),
        Err(RepositoryError::AggregateLimit)
    );
}

#[test]
fn snapshot_and_nested_dto_json_names_match_the_frontend_contract() {
    let value =
        serde_json::to_value(crate::model::Snapshot::default()).expect("serialize snapshot");
    let object = value.as_object().expect("snapshot object");
    for key in [
        "state",
        "root",
        "gitDir",
        "commonGitDir",
        "bare",
        "linkedWorktree",
        "status",
        "remotes",
        "localBranches",
        "remoteBranches",
        "recentCommits",
        "unpushedCommits",
        "recentCommitsTruncated",
        "unpushedCommitsTruncated",
        "worktrees",
        "worktreeBounds",
        "worktreesIncomplete",
        "staleBranchPolicy",
    ] {
        assert!(object.contains_key(key), "missing JSON key {key}");
    }
    assert!(!object.contains_key("divergence"));
    let status = object["status"].as_object().expect("status object");
    assert!(status.contains_key("changedPathBounds"));
    assert!(status.contains_key("untrackedPaths"));
    for key in ["changedPaths", "untrackedPaths"] {
        assert!(status[key].is_null(), "zero Go slice {key} must be null");
    }
    for key in [
        "remotes",
        "localBranches",
        "remoteBranches",
        "recentCommits",
        "unpushedCommits",
        "worktrees",
    ] {
        assert!(object[key].is_null(), "zero Go slice {key} must be null");
    }
}

#[test]
fn capture_json_preserves_tolerated_nulls_and_successful_empty_arrays() {
    let runner = Arc::new(FakeRunner::default());
    seed_minimal_capture(&runner, "/repo");
    runner.error("/repo|worktree", RepositoryError::CommandFailed);
    runner.error("/repo|config", RepositoryError::CommandFailed);
    runner.error("/repo|log", RepositoryError::CommandFailed);
    let snapshot = RepositoryService::with_runner_and_clock(runner, now)
        .capture(&CancellationToken::new(), "/repo")
        .expect("capture tolerated empty snapshot");
    let value = serde_json::to_value(snapshot).expect("serialize snapshot");
    for key in ["worktrees", "remotes", "recentCommits", "unpushedCommits"] {
        assert!(value[key].is_null(), "tolerated absence {key} must be null");
    }
    for key in ["localBranches", "remoteBranches"] {
        assert_eq!(value[key], serde_json::json!([]));
    }
    assert_eq!(value["status"]["changedPaths"], serde_json::json!([]));
    assert_eq!(value["status"]["untrackedPaths"], serde_json::json!([]));
}

#[test]
fn push_only_remote_json_keeps_nil_fetch_urls() {
    let runner = Arc::new(FakeRunner::default());
    seed_minimal_capture(&runner, "/repo");
    runner.error("/repo|worktree", RepositoryError::CommandFailed);
    runner.output(
        "/repo|config",
        b"remote.origin.pushurl\nssh://example.test/push.git\0".to_vec(),
    );
    runner.error("/repo|log", RepositoryError::CommandFailed);
    let snapshot = RepositoryService::with_runner_and_clock(runner, now)
        .capture(&CancellationToken::new(), "/repo")
        .expect("capture push-only remote");
    let value = serde_json::to_value(snapshot).expect("serialize snapshot");
    assert!(value["remotes"][0]["fetchUrls"].is_null());
    assert_eq!(
        value["remotes"][0]["pushUrls"],
        serde_json::json!(["ssh://example.test/push.git"])
    );
}

#[test]
fn real_disposable_repository_handles_initial_and_committed_states() {
    if std::process::Command::new("git")
        .arg("--version")
        .status()
        .is_err()
    {
        return;
    }
    let root = temp_directory("ptrack-git-real");
    run_git(&root, &["init", "-q"]);
    let service = RepositoryService::new();
    let initial = service
        .capture(&CancellationToken::new(), &root)
        .expect("capture initial repository");
    assert_eq!(initial.state, RepositoryState::Ready);
    assert_eq!(initial.status.oid, "(initial)");
    assert!(initial.recent_commits.is_none());

    run_git(&root, &["config", "user.name", "P Track"]);
    run_git(&root, &["config", "user.email", "ptrack@example.test"]);
    std::fs::write(root.join("tracked.txt"), b"one\n").expect("write tracked file");
    run_git(&root, &["add", "tracked.txt"]);
    run_git(&root, &["commit", "-q", "-m", "initial"]);
    std::fs::write(root.join("untracked.txt"), b"two\n").expect("write untracked file");
    let snapshot = service
        .capture(&CancellationToken::new(), &root)
        .expect("capture committed repository");
    assert_eq!(snapshot.root, canonical(&root).to_string_lossy());
    assert_eq!(snapshot.status.untracked, 1);
    let recent_commits = snapshot.recent_commits.expect("log succeeded");
    assert_eq!(recent_commits.len(), 1);
    assert_eq!(recent_commits[0].subject, "initial");
    std::fs::remove_dir_all(root).expect("remove real repository");
}

#[cfg(unix)]
#[test]
fn malicious_repository_fsmonitor_is_never_executed() {
    use std::os::unix::fs::PermissionsExt;

    let root = temp_directory("ptrack-git-fsmonitor");
    run_git(&root, &["init", "-q"]);
    run_git(&root, &["config", "user.name", "P Track"]);
    run_git(&root, &["config", "user.email", "ptrack@example.test"]);
    std::fs::write(root.join("tracked.txt"), b"one\n").expect("write tracked file");
    run_git(&root, &["add", "tracked.txt"]);
    run_git(&root, &["commit", "-q", "-m", "initial"]);
    let marker = root.join("fsmonitor-executed");
    let hook = root.join("malicious-fsmonitor");
    std::fs::write(
        &hook,
        format!("#!/bin/sh\ntouch '{}'\nexit 1\n", marker.display()),
    )
    .expect("write malicious fsmonitor");
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o700))
        .expect("make fsmonitor executable");
    run_git(
        &root,
        &[
            "config",
            "core.fsmonitor",
            hook.to_str().expect("hook utf8"),
        ],
    );

    RepositoryService::new()
        .capture(&CancellationToken::new(), &root)
        .expect("hardened capture");
    assert!(!marker.exists(), "repository-controlled fsmonitor executed");
    std::fs::remove_dir_all(root).expect("remove fsmonitor repository");
}

#[test]
fn capture_skips_divergence_when_the_upstream_did_not_resolve() {
    let runner_with_status = Arc::new(FakeRunner::default());
    runner_with_status.output(
        "/repo|rev-parse",
        b"true\n/repo\n/repo/.git\n/repo/.git\nfalse\n".to_vec(),
    );
    runner_with_status.output(
        "/repo|status",
        b"# branch.oid (initial)\0# branch.head main\0# branch.upstream origin/main\0".to_vec(),
    );
    runner_with_status.output("/repo|for-each-ref:refs/heads", Vec::new());
    runner_with_status.output("/repo|for-each-ref:refs/remotes", Vec::new());
    let snapshot = RepositoryService::with_runner_and_clock(runner_with_status.clone(), now)
        .capture(&CancellationToken::new(), "/repo")
        .expect("a missing upstream must not fail the snapshot");
    assert_eq!(snapshot.status.upstream, "origin/main");
    assert!(snapshot.divergence.is_none());
    assert!(snapshot.unpushed_commits.is_none());
    assert!(
        runner_with_status
            .calls()
            .iter()
            .all(|(_, args)| args.first().is_none_or(|command| command != "rev-list"))
    );
}

#[test]
fn capture_tolerates_a_failed_divergence_query() {
    let runner = Arc::new(FakeRunner::default());
    runner.output(
        "/repo|rev-parse",
        b"true\n/repo\n/repo/.git\n/repo/.git\nfalse\n".to_vec(),
    );
    runner.output(
        "/repo|status",
        b"# branch.oid abc\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +1 -0\0"
            .to_vec(),
    );
    runner.output("/repo|for-each-ref:refs/heads", Vec::new());
    runner.output("/repo|for-each-ref:refs/remotes", Vec::new());
    runner.error("/repo|rev-list", RepositoryError::CommandFailed);
    let snapshot = RepositoryService::with_runner_and_clock(runner, now)
        .capture(&CancellationToken::new(), "/repo")
        .expect("a vanished upstream must not fail the snapshot");
    assert!(snapshot.divergence.is_none());
    assert_eq!(snapshot.status.ahead, 1);
}

#[test]
fn capture_counts_ignored_entries_in_a_separate_non_fatal_query() {
    let runner = Arc::new(FakeRunner::default());
    seed_minimal_capture(&runner, "/repo");
    runner.output("/repo|ls-files", b"build/\0debug.log\0".to_vec());
    let snapshot = RepositoryService::with_runner_and_clock(runner, now)
        .capture(&CancellationToken::new(), "/repo")
        .expect("capture with ignored entries");
    assert_eq!(snapshot.status.ignored, 2);

    for error in [
        RepositoryError::OutputLimit,
        RepositoryError::CommandTimeout,
        RepositoryError::CommandFailed,
    ] {
        let runner = Arc::new(FakeRunner::default());
        seed_minimal_capture(&runner, "/repo");
        runner.error("/repo|ls-files", error);
        let snapshot = RepositoryService::with_runner_and_clock(runner, now)
            .capture(&CancellationToken::new(), "/repo")
            .expect("an ignored-count failure must not fail the snapshot");
        assert_eq!(snapshot.status.ignored, 0);
    }
}

#[test]
fn capture_keeps_commits_with_separator_bytes_and_non_utf8_authors() {
    let runner = Arc::new(FakeRunner::default());
    seed_minimal_capture(&runner, "/repo");
    let mut log = Vec::new();
    log.extend_from_slice(format!("\0{}\0", "a".repeat(40)).as_bytes());
    log.extend_from_slice(b"Jos\xe9\0jose@example.test\x001785060000\0split\x1ehere\x1fand here\0HEAD -> main\0\n\nsrc/a.rs\n");
    log.extend_from_slice(
        log_record(
            &"b".repeat(40),
            "Ada",
            "ada@example.test",
            1_785_000_000,
            "plain",
            "",
            &["README.md"],
        )
        .as_bytes(),
    );
    log.extend_from_slice(b"\0not-a-sha\0X\0x@example.test\x001785000000\0bad\0\0\n");
    runner.output("/repo|log", log);
    let snapshot = RepositoryService::with_runner_and_clock(runner, now)
        .capture(&CancellationToken::new(), "/repo")
        .expect("unusual commit metadata must not fail the snapshot");
    let commits = snapshot.recent_commits.expect("log parsed");
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[0].author_name, "Jos\u{fffd}");
    assert_eq!(commits[0].subject, "split\u{1e}here\u{1f}and here");
    assert_eq!(commits[0].refs, ["HEAD -> main"]);
    assert_eq!(commits[0].files_changed, 1);
    assert_eq!(commits[1].subject, "plain");
}

#[test]
fn capture_redacts_remote_url_credentials() {
    let runner = Arc::new(FakeRunner::default());
    seed_minimal_capture(&runner, "/repo");
    runner.output(
        "/repo|config",
        b"remote.origin.url\nhttps://user:ghp_secret@github.com/o/r.git\0\
          remote.origin.pushurl\nssh://git@github.com/o/r.git\0\
          remote.token.url\nhttps://glpat-secret@gitlab.example/o/r.git\0\
          remote.scp.url\ngit@github.com:o/r.git\0\
          remote.odd.url\nhttps://h\xffst/o/r.git\0"
            .to_vec(),
    );
    let snapshot = RepositoryService::with_runner_and_clock(runner, now)
        .capture(&CancellationToken::new(), "/repo")
        .expect("capture remotes");
    let remotes = snapshot.remotes.expect("remotes parsed");
    let urls: Vec<String> = remotes
        .iter()
        .flat_map(|remote| {
            remote
                .fetch_urls
                .clone()
                .unwrap_or_default()
                .into_iter()
                .chain(remote.push_urls.clone())
        })
        .collect();
    assert!(urls.iter().all(|url| !url.contains("secret")), "{urls:?}");
    let origin = remotes
        .iter()
        .find(|remote| remote.name == "origin")
        .unwrap();
    assert_eq!(
        origin.fetch_urls.as_deref(),
        Some(&["https://github.com/o/r.git".to_owned()][..])
    );
    assert_eq!(origin.push_urls, ["ssh://git@github.com/o/r.git"]);
    let scp = remotes.iter().find(|remote| remote.name == "scp").unwrap();
    assert_eq!(scp.push_urls, ["git@github.com:o/r.git"]);
    assert!(remotes.iter().any(|remote| remote.name == "odd"));
}

#[test]
fn remote_url_redaction_covers_userinfo_forms() {
    for (input, expected) in [
        ("https://github.com/o/r.git", "https://github.com/o/r.git"),
        ("https://user:pass@host/o/r", "https://host/o/r"),
        ("https://token@host", "https://host"),
        ("http://a:b@host:8080/p?q=1", "http://host:8080/p?q=1"),
        ("ssh://git@host/o/r", "ssh://git@host/o/r"),
        ("ssh://git:secret@host/o/r", "ssh://host/o/r"),
        ("git@host:o/r.git", "git@host:o/r.git"),
        ("https://host/path@with-at", "https://host/path@with-at"),
        ("file:///srv/repo.git", "file:///srv/repo.git"),
    ] {
        assert_eq!(
            crate::snapshot::redact_url_userinfo(input),
            expected,
            "{input}"
        );
    }
}

#[test]
fn capture_skips_refs_that_are_not_utf8_or_lack_a_commit_date() {
    let mut remotes = Vec::new();
    remotes.extend_from_slice(b"refs/remotes/origin/caf\xe9\0abc\0\x001785000000\0 \0\0\n");
    remotes.extend_from_slice(b"refs/remotes/origin/blob\0def\0\0\0 \0\0\n");
    remotes.extend_from_slice(
        ref_record(
            "refs/remotes/origin/main",
            "123",
            "",
            1_785_000_000,
            " ",
            "",
        )
        .as_bytes(),
    );
    let runner = Arc::new(FakeRunner::default());
    runner.output(
        "/repo|rev-parse",
        b"true\n/repo\n/repo/.git\n/repo/.git\nfalse\n".to_vec(),
    );
    runner.output("/repo|status", Vec::new());
    runner.output("/repo|for-each-ref:refs/heads", Vec::new());
    runner.output("/repo|for-each-ref:refs/remotes", remotes);
    let snapshot = RepositoryService::with_runner_and_clock(runner, now)
        .capture(&CancellationToken::new(), "/repo")
        .expect("unusual refs must not fail the snapshot");
    let remote_branches = snapshot.remote_branches.expect("remote refs parsed");
    assert_eq!(remote_branches.len(), 1);
    assert_eq!(remote_branches[0].name, "origin/main");
}

#[test]
fn real_repository_without_a_resolved_upstream_still_captures() {
    if std::process::Command::new("git")
        .arg("--version")
        .status()
        .is_err()
    {
        return;
    }
    let base = temp_directory("ptrack-git-upstream");
    let remote = base.join("remote.git");
    std::fs::create_dir(&remote).expect("create bare remote directory");
    run_git(&remote, &["init", "-q", "--bare"]);

    // A fresh clone of an empty repository tracks a branch that does not
    // exist yet.
    run_git(
        &base,
        &[
            "clone",
            "-q",
            remote.to_str().expect("remote path utf8"),
            "empty-clone",
        ],
    );
    let empty = base.join("empty-clone");
    let snapshot = RepositoryService::new()
        .capture(&CancellationToken::new(), &empty)
        .expect("capture fresh clone of an empty repository");
    assert_eq!(snapshot.state, RepositoryState::Ready);
    assert!(snapshot.divergence.is_none());

    // A branch whose remote-tracking ref was pruned keeps its upstream
    // configuration but has nothing to diverge from.
    let work = base.join("work");
    std::fs::create_dir(&work).expect("create work repository");
    run_git(&work, &["init", "-q"]);
    run_git(&work, &["config", "user.name", "P Track"]);
    run_git(&work, &["config", "user.email", "ptrack@example.test"]);
    run_git(&work, &["config", "commit.gpgsign", "false"]);
    std::fs::write(work.join("tracked.txt"), b"one\n").expect("write tracked file");
    run_git(&work, &["add", "tracked.txt"]);
    run_git(&work, &["commit", "-q", "-m", "one\u{1e}two\u{1f}three"]);
    run_git(&work, &["checkout", "-q", "-b", "feature"]);
    run_git(
        &work,
        &[
            "remote",
            "add",
            "origin",
            remote.to_str().expect("remote path utf8"),
        ],
    );
    run_git(&work, &["push", "-q", "-u", "origin", "feature"]);
    run_git(&work, &["update-ref", "-d", "refs/remotes/origin/feature"]);
    std::fs::write(work.join(".gitignore"), b"*.log\n").expect("write ignore file");
    std::fs::write(work.join("debug.log"), b"noise\n").expect("write ignored file");
    let snapshot = RepositoryService::new()
        .capture(&CancellationToken::new(), &work)
        .expect("capture branch with a pruned upstream");
    assert_eq!(snapshot.status.upstream, "origin/feature");
    assert!(snapshot.divergence.is_none());
    assert_eq!(snapshot.status.ignored, 1);
    let commits = snapshot.recent_commits.expect("log succeeded");
    assert_eq!(commits[0].subject, "one\u{1e}two\u{1f}three");
    std::fs::remove_dir_all(base).expect("remove upstream test repositories");
}

fn ref_record(
    reference: &str,
    oid: &str,
    upstream: &str,
    epoch: i64,
    head: &str,
    worktree: &str,
) -> String {
    format!("{reference}\0{oid}\0{upstream}\0{epoch}\0{head}\0{worktree}\0\n")
}

fn log_record(
    sha: &str,
    author: &str,
    email: &str,
    epoch: i64,
    subject: &str,
    refs: &str,
    paths: &[&str],
) -> String {
    format!(
        "\0{sha}\0{author}\0{email}\0{epoch}\0{subject}\0{refs}\0\n\n{}\n",
        paths.join("\n")
    )
}

fn seed_minimal_capture(runner: &FakeRunner, root: &str) {
    runner.output(
        &format!("{root}|rev-parse"),
        format!("true\n{root}\n{root}/.git\n{root}/.git\nfalse\n"),
    );
    runner.output(&format!("{root}|status"), Vec::new());
    runner.output(&format!("{root}|for-each-ref:refs/heads"), Vec::new());
    runner.output(&format!("{root}|for-each-ref:refs/remotes"), Vec::new());
}

fn temp_directory(prefix: &str) -> std::path::PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let directory = std::env::temp_dir().join(format!(
        "{prefix}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&directory).expect("create test directory");
    directory
}
