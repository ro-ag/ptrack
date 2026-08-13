use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::process::Command;
use std::time::Duration;
use std::time::Instant;

use tokio_util::sync::CancellationToken;

use super::process::{ProcessError, ProcessSpec, run_process, safe_environment};

#[test]
#[ignore = "subprocess fixture"]
fn subprocess_writes_both_streams() {
    let chunk = vec![b'x'; 32 * 1_024];
    let mut stdout = io::stdout().lock();
    let mut stderr = io::stderr().lock();
    for _ in 0..8 {
        stdout.write_all(&chunk).unwrap();
        stdout.flush().unwrap();
        stderr.write_all(&chunk).unwrap();
        stderr.flush().unwrap();
    }
}

#[test]
#[ignore = "subprocess fixture"]
fn subprocess_sleeps() {
    std::thread::sleep(Duration::from_secs(30));
}

#[test]
#[ignore = "subprocess fixture"]
fn subprocess_prints_argument() {
    println!("{}", std::env::args().next_back().unwrap());
}

#[test]
#[ignore = "subprocess fixture"]
fn subprocess_spawns_pipe_holder() {
    let marker = std::env::args().next_back().unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "process_test::subprocess_holds_pipes_then_marks",
            "--nocapture",
            "--",
            &marker,
        ])
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_secs(30));
    child.wait().unwrap();
}

#[test]
#[ignore = "subprocess fixture"]
fn subprocess_holds_pipes_then_marks() {
    let marker = std::env::args().next_back().unwrap();
    std::thread::sleep(Duration::from_millis(250));
    fs::write(marker, b"descendant survived").unwrap();
}

#[tokio::test]
async fn process_uses_one_concurrent_output_budget() {
    let result = run_process(
        &fixture_spec(
            "subprocess_writes_both_streams",
            &[],
            12_345,
            Duration::from_secs(5),
        ),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(result.stdout.len() + result.stderr.len(), 12_345);
    assert!(result.truncated);
}

#[tokio::test]
async fn process_timeout_and_cancel_kill_and_reap() {
    let timeout = run_process(
        &fixture_spec("subprocess_sleeps", &[], 1_024, Duration::from_millis(30)),
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(timeout, ProcessError::Timeout);

    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let cancelled = run_process(
        &fixture_spec("subprocess_sleeps", &[], 1_024, Duration::from_secs(5)),
        &cancellation,
    )
    .await
    .unwrap_err();
    assert_eq!(cancelled, ProcessError::Cancelled);
}

#[tokio::test]
async fn descendant_pipe_holders_cannot_extend_deadline_or_exhaust_reader_permits() {
    let marker =
        std::env::temp_dir().join(format!("ptrack-process-descendant-{}", std::process::id()));
    let _ = fs::remove_file(&marker);
    let marker_text = marker.to_str().unwrap();
    let started = Instant::now();
    for _ in 0..=16 {
        let error = run_process(
            &fixture_spec(
                "subprocess_spawns_pipe_holder",
                &[marker_text],
                1_024,
                Duration::from_millis(40),
            ),
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
        assert_eq!(error, ProcessError::Timeout);
    }
    assert!(started.elapsed() < Duration::from_secs(3));

    let recovery = run_process(
        &fixture_spec(
            "subprocess_prints_argument",
            &["permit-recovered"],
            1_024,
            Duration::from_secs(1),
        ),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(
        String::from_utf8(recovery.stdout)
            .unwrap()
            .lines()
            .any(|line| line == "permit-recovered")
    );
    tokio::time::sleep(Duration::from_millis(350)).await;
    assert!(
        !marker.exists(),
        "descendant survived process-tree teardown"
    );
}

#[tokio::test]
async fn process_passes_argv_without_shell_interpretation() {
    let hostile = "$(printf exploited);`printf exploited`;*";
    let result = run_process(
        &fixture_spec(
            "subprocess_prints_argument",
            &[hostile],
            1_024,
            Duration::from_secs(5),
        ),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(
        String::from_utf8(result.stdout)
            .unwrap()
            .lines()
            .any(|line| line == hostile)
    );
}

#[test]
fn process_environment_scrubs_all_ambient_git_authority() {
    let environment = safe_environment(&[
        (OsString::from("GIT_TERMINAL_PROMPT"), OsString::from("0")),
        (OsString::from("LC_ALL"), OsString::from("C")),
    ]);
    assert_eq!(
        environment.get(&OsString::from("GIT_TERMINAL_PROMPT")),
        Some(&OsString::from("0"))
    );
    assert_eq!(
        environment.get(&OsString::from("LC_ALL")),
        Some(&OsString::from("C"))
    );
    assert!(environment.keys().all(|name| {
        name == "GIT_TERMINAL_PROMPT"
            || !name
                .to_string_lossy()
                .to_ascii_uppercase()
                .starts_with("GIT_")
    }));
}

fn fixture_spec(fixture: &str, arguments: &[&str], maximum: u64, timeout: Duration) -> ProcessSpec {
    let mut args: Vec<OsString> = vec![
        "--ignored".into(),
        "--exact".into(),
        format!("process_test::{fixture}").into(),
        "--nocapture".into(),
        "--".into(),
    ];
    args.extend(arguments.iter().map(OsString::from));
    ProcessSpec {
        name: std::env::current_exe().unwrap().into_os_string(),
        args,
        env: Vec::new(),
        max_output_bytes: maximum,
        timeout,
    }
}
