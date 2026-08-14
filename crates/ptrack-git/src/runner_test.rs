use std::ffi::OsString;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::runner::{
    CancellationToken, ExecRunner, RepositoryError, Runner, git_command_args, git_environment,
};

#[test]
fn git_command_is_hardened_and_root_scoped() {
    let got = git_command_args(
        Path::new("/project"),
        &[OsString::from("status"), OsString::from("--porcelain=v2")],
    );
    let want: Vec<OsString> = [
        "--no-optional-locks",
        "-c",
        "core.fsmonitor=false",
        "-C",
        "/project",
        "status",
        "--porcelain=v2",
    ]
    .map(OsString::from)
    .into();
    assert_eq!(got, want);
}

#[test]
fn git_environment_scrubs_case_insensitively_and_appends_fixed_values() {
    let source = [
        ("PATH", "/bin"),
        ("lang", "fr_FR"),
        ("Git_Dir", "/attacker/repository"),
        ("GIT_WORK_TREE", "/attacker/worktree"),
        ("gIt_CoNfIg_CoUnT", "1"),
        ("GIT_NO_LAZY_FETCH", "0"),
    ]
    .map(|(key, value)| (OsString::from(key), OsString::from(value)));
    let environment = git_environment(source);
    assert_eq!(
        environment[0],
        (OsString::from("PATH"), OsString::from("/bin"))
    );
    let fixed: Vec<(OsString, OsString)> = [
        ("LANG", "C"),
        ("LC_ALL", "C"),
        ("GIT_OPTIONAL_LOCKS", "0"),
        ("GIT_PAGER", "cat"),
        ("GIT_TERMINAL_PROMPT", "0"),
        ("GIT_NO_LAZY_FETCH", "1"),
    ]
    .map(|(key, value)| (OsString::from(key), OsString::from(value)))
    .into();
    assert_eq!(&environment[1..], fixed);
    assert!(environment.iter().all(|(key, value)| {
        !key.to_string_lossy().eq_ignore_ascii_case("Git_Dir")
            && !value.to_string_lossy().contains("attacker")
    }));
}

#[test]
fn cancelled_runner_does_not_spawn() {
    let token = CancellationToken::new();
    token.cancel();
    let error = ExecRunner::default()
        .output(&token, Path::new("/project"), &[OsString::from("status")])
        .expect_err("cancelled runner must fail");
    assert_eq!(error, RepositoryError::Cancelled);
}

#[test]
fn reader_capacity_exhaustion_fails_closed() {
    let runner = ExecRunner::without_reader_capacity_for_test(
        std::env::current_exe().expect("current test executable"),
        Duration::from_secs(1),
    );
    assert_eq!(
        runner.output(
            &CancellationToken::new(),
            Path::new("."),
            &[OsString::from("status")]
        ),
        Err(RepositoryError::CommandFailed)
    );
}

#[cfg(unix)]
#[test]
fn runner_bounds_combined_output_returns_stdout_only_and_never_uses_root_as_cwd() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile_dir("ptrack-git-runner-output");
    let script = directory.join("fake-git");
    std::fs::write(
        &script,
        "#!/bin/sh\nprintf 'stdout'\nprintf 'stderr' >&2\npwd\n",
    )
    .expect("write fake git");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))
        .expect("make fake git executable");
    let runner = ExecRunner::for_test(&script, Duration::from_secs(1), 1024);
    let output = runner
        .output(
            &CancellationToken::new(),
            &directory.join("not-the-cwd"),
            &[OsString::from("status")],
        )
        .expect("fake git succeeds");
    let text = String::from_utf8(output).expect("stdout utf8");
    assert!(text.starts_with("stdout"));
    assert!(!text.contains("stderr"));
    assert!(!text.contains("not-the-cwd"));

    let bounded = ExecRunner::for_test(&script, Duration::from_secs(1), 8);
    assert_eq!(
        bounded.output(
            &CancellationToken::new(),
            &directory,
            &[OsString::from("status")]
        ),
        Err(RepositoryError::OutputLimit)
    );
    std::fs::remove_dir_all(directory).expect("remove runner test directory");
}

#[cfg(unix)]
#[test]
fn runner_times_out_kills_and_reaps_child() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile_dir("ptrack-git-runner-timeout");
    let script = directory.join("fake-git");
    std::fs::write(&script, "#!/bin/sh\nexec sleep 30\n").expect("write fake git");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))
        .expect("make fake git executable");
    let runner = ExecRunner::for_test(&script, Duration::from_millis(25), 1024);
    assert_eq!(
        runner.output(
            &CancellationToken::new(),
            &directory,
            &[OsString::from("status")]
        ),
        Err(RepositoryError::CommandTimeout)
    );
    std::fs::remove_dir_all(directory).expect("remove runner test directory");
}

#[cfg(unix)]
#[test]
fn runner_deadline_is_not_extended_by_descendant_inheriting_pipes() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile_dir("ptrack-git-runner-descendant");
    let script = directory.join("fake-git");
    std::fs::write(
        &script,
        "#!/bin/sh\n(sleep 1; printf descendant) &\nexit 0\n",
    )
    .expect("write fake git");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))
        .expect("make fake git executable");
    let runner = ExecRunner::for_test(&script, Duration::from_millis(30), 1024);
    let started = Instant::now();
    assert_eq!(
        runner.output(
            &CancellationToken::new(),
            &directory,
            &[OsString::from("status")]
        ),
        Err(RepositoryError::CommandTimeout)
    );
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "runner blocked on a descendant-owned pipe"
    );
    std::thread::sleep(Duration::from_millis(1_050));
    std::fs::remove_dir_all(directory).expect("remove runner test directory");
}

#[cfg(windows)]
#[test]
fn runner_deadline_is_not_extended_by_descendant_inheriting_pipes() {
    let directory = tempfile_dir("ptrack-git-runner-descendant");
    let script = directory.join("fake-git.cmd");
    std::fs::write(
        &script,
        "@echo off\r\nstart \"\" /b cmd /d /c \"ping 127.0.0.1 -n 2 >nul & echo descendant\"\r\nexit /b 0\r\n",
    )
    .expect("write fake git batch file");
    let runner = ExecRunner::for_test(&script, Duration::from_millis(30), 1024);
    let started = Instant::now();
    assert_eq!(
        runner.output(
            &CancellationToken::new(),
            &directory,
            &[OsString::from("status")]
        ),
        Err(RepositoryError::CommandTimeout)
    );
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "runner blocked on a descendant-owned pipe"
    );
    std::thread::sleep(Duration::from_millis(1_100));
    std::fs::remove_dir_all(directory).expect("remove runner test directory");
}

#[cfg(any(unix, windows))]
fn tempfile_dir(prefix: &str) -> std::path::PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let directory = std::env::temp_dir().join(format!(
        "{prefix}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&directory).expect("create runner test directory");
    directory
}
