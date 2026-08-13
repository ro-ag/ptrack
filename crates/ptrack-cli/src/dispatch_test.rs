use std::io::{Read, Write};
use std::path::PathBuf;

use ptrack_app::{
    AppError, AppResult, ApplicationPort, CapabilityMcpOutcome, GuideAction, HookAction,
    HookResult, InitRequest, InitResult, Mutation, MutationResult, ProcessOutput,
};
use ptrack_core::{Meta, ProjectRef, ProjectSnapshot, Timestamp};

use crate::{Io, RunOutcome, run};

struct FakeApplication {
    snapshot: ProjectSnapshot,
    git_output: Option<ProcessOutput>,
}

impl Default for FakeApplication {
    fn default() -> Self {
        Self {
            snapshot: ProjectSnapshot::new(
                Meta {
                    goal: "goal <x>".to_owned(),
                    summary: String::new(),
                    active_plan: 0,
                    created_at: Timestamp::Zero,
                    updated_at: Timestamp::Zero,
                    format_version: 5,
                    last_write_version: "test".to_owned(),
                },
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            ),
            git_output: None,
        }
    }
}

impl ApplicationPort for FakeApplication {
    fn initialize(&mut self, _request: InitRequest) -> AppResult<InitResult> {
        Err(AppError::NotImplemented("test initialize"))
    }

    fn snapshot(&mut self) -> AppResult<ProjectSnapshot> {
        Ok(self.snapshot.clone())
    }

    fn mutate(&mut self, mutation: Mutation) -> AppResult<MutationResult> {
        match mutation {
            Mutation::SetGoal(value) => self.snapshot.meta.goal = value,
            Mutation::SetSummary(value) => self.snapshot.meta.summary = value,
            _ => return Err(AppError::NotImplemented("test mutation")),
        }
        Ok(MutationResult::None)
    }

    fn projects(&mut self) -> AppResult<Vec<ProjectRef>> {
        Ok(Vec::new())
    }

    fn backup(&mut self) -> AppResult<PathBuf> {
        Err(AppError::NotImplemented("test backup"))
    }

    fn guide(&mut self, _action: GuideAction) -> AppResult<(String, Vec<PathBuf>)> {
        Err(AppError::NotImplemented("test guide"))
    }

    fn hook(&mut self, _action: HookAction) -> AppResult<HookResult> {
        Err(AppError::NotImplemented("test hook"))
    }

    fn git_show(&mut self, _reference: &str, _stat: bool) -> AppResult<ProcessOutput> {
        self.git_output
            .take()
            .ok_or(AppError::NotImplemented("test git"))
    }

    fn capability_call(&mut self, _tool: &str, _arguments: &str) -> AppResult<Vec<u8>> {
        Err(AppError::NotImplemented("test capability"))
    }

    fn capability_mcp(
        &mut self,
        _input: &mut dyn Read,
        _output: &mut dyn Write,
    ) -> AppResult<CapabilityMcpOutcome> {
        Err(AppError::NotImplemented("test mcp"))
    }
}

fn invoke(args: &[&str]) -> (Result<RunOutcome, crate::CliError>, String, String) {
    let mut application = FakeApplication::default();
    invoke_with(&mut application, args)
}

fn invoke_with(
    application: &mut FakeApplication,
    args: &[&str],
) -> (Result<RunOutcome, crate::CliError>, String, String) {
    let mut input = std::io::empty();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let result = run(
        args.iter().map(|value| (*value).to_owned()),
        application,
        Io {
            stdin: &mut input,
            stdout: &mut stdout,
            stderr: &mut stderr,
        },
    );
    (
        result,
        String::from_utf8(stdout).expect("stdout utf8"),
        String::from_utf8(stderr).expect("stderr utf8"),
    )
}

#[test]
fn commit_show_preserves_git_stderr_and_actual_exit_code() {
    let mut application = FakeApplication {
        git_output: Some(ProcessOutput {
            stdout: b"partial\n".to_vec(),
            stderr: b"fatal: bad object\n".to_vec(),
            exit_code: Some(42),
        }),
        ..FakeApplication::default()
    };
    let (result, stdout, stderr) =
        invoke_with(&mut application, &["ptrack", "commit", "show", "bad"]);
    assert_eq!(stdout, "partial\n");
    assert_eq!(stderr, "fatal: bad object\n");
    assert_eq!(
        result.expect_err("git failed").to_string(),
        "exit status 42"
    );
}

#[test]
fn process_routing_and_stream_ownership_are_explicit() {
    let (result, stdout, stderr) = invoke(&["ptrack"]);
    assert_eq!(result.expect("no args"), RunOutcome::LaunchTui);
    assert!(stdout.is_empty());
    assert!(stderr.is_empty());

    let (result, stdout, stderr) = invoke(&["ptrack", "gui", "repo"]);
    assert_eq!(
        result.expect("gui"),
        RunOutcome::LaunchGui {
            path: "repo".to_owned(),
            plan_id: 0,
        }
    );
    assert!(stdout.is_empty());
    assert!(stderr.is_empty());
}

#[test]
fn help_and_unknown_command_match_the_go_process_contract() {
    let (result, stdout, stderr) = invoke(&["ptrack", "--help"]);
    assert_eq!(result.expect("help"), RunOutcome::ExitSuccess);
    assert!(stdout.starts_with("p-track keeps project plans alive"));
    assert!(stdout.contains("  completion  Generate the autocompletion script"));
    assert!(stderr.is_empty());

    let (result, stdout, stderr) = invoke(&["ptrack", "milestne"]);
    assert!(stdout.is_empty());
    assert!(stderr.is_empty());
    assert_eq!(
        result.expect_err("unknown").to_string(),
        "unknown command \"milestne\" for \"ptrack\"\n\nDid you mean this?\n\tmilestone\n"
    );
}

#[test]
fn status_json_uses_go_key_order_and_html_escaping() {
    let (result, stdout, stderr) = invoke(&["ptrack", "status", "--json"]);
    assert_eq!(result.expect("status"), RunOutcome::ExitSuccess);
    assert!(stderr.is_empty());
    assert_eq!(
        stdout,
        "{\n  \"goal\": \"goal \\u003cx\\u003e\",\n  \"active_plan\": 0,\n  \"active_plan_title\": \"\",\n  \"plans\": 0,\n  \"todo\": 0,\n  \"doing\": 0,\n  \"done\": 0,\n  \"blocked\": 0\n}\n"
    );
}

#[test]
fn missing_report_roots_use_the_go_not_found_error() {
    for args in [
        ["ptrack", "milestone", "show", "99"].as_slice(),
        ["ptrack", "plan", "show", "99"].as_slice(),
        ["ptrack", "task", "show", "99"].as_slice(),
        ["ptrack", "issue", "show", "99"].as_slice(),
        ["ptrack", "board", "--plan", "99"].as_slice(),
    ] {
        let (result, stdout, stderr) = invoke(args);
        assert!(stdout.is_empty(), "unexpected stdout for {args:?}");
        assert!(stderr.is_empty(), "unexpected stderr for {args:?}");
        assert_eq!(result.expect_err("missing root").to_string(), "not found");
    }
}

#[test]
fn due_parse_errors_wrap_the_exact_go_time_parse_error() {
    for (args, expected) in [
        (
            ["ptrack", "milestone", "add", "x", "--due", "2024-1-2"].as_slice(),
            "invalid --due \"2024-1-2\" (want YYYY-MM-DD): parsing time \"2024-1-2\" as \"2006-01-02\": cannot parse \"1-2\" as \"01\"",
        ),
        (
            ["ptrack", "milestone", "due", "1", "2024-02-30"].as_slice(),
            "invalid date \"2024-02-30\" (want YYYY-MM-DD): parsing time \"2024-02-30\": day out of range",
        ),
    ] {
        let (result, stdout, stderr) = invoke(args);
        assert!(stdout.is_empty());
        assert!(stderr.is_empty());
        assert_eq!(result.expect_err("invalid due").to_string(), expected);
    }
}

#[test]
fn help_errors_and_guide_are_byte_differential_against_go() {
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repository root")
        .to_path_buf();
    let go = repository.join("build/bin/ptrack");
    if !go.is_file() {
        return;
    }

    for args in [
        vec!["ptrack", "--help"],
        vec!["ptrack", "task", "--help"],
        vec!["ptrack", "task", "add", "--help"],
    ] {
        let (_, rust_stdout, rust_stderr) = invoke(&args);
        let baseline = std::process::Command::new(&go)
            .args(&args[1..])
            .output()
            .expect("run Go baseline");
        assert!(baseline.status.success());
        assert_eq!(rust_stdout.as_bytes(), baseline.stdout);
        assert_eq!(rust_stderr.as_bytes(), baseline.stderr);
    }

    let (result, rust_stdout, rust_stderr) = invoke(&["ptrack", "milestne"]);
    let baseline = std::process::Command::new(&go)
        .arg("milestne")
        .output()
        .expect("run Go unknown baseline");
    assert!(!baseline.status.success());
    assert!(rust_stdout.is_empty());
    assert!(rust_stderr.is_empty());
    assert_eq!(
        format!("{}\n", result.expect_err("Rust unknown")),
        String::from_utf8(baseline.stderr).expect("Go stderr utf8")
    );

    let isolated_home = repository.join("target/cli-go-guide-home-does-not-exist");
    let baseline = std::process::Command::new(go)
        .args(["guide", "--print"])
        .env("PTRACK_HOME", isolated_home)
        .output()
        .expect("run Go guide baseline");
    assert!(baseline.status.success());
    assert_eq!(
        ptrack_core::render_guide("").as_bytes(),
        baseline.stdout,
        "Rust guide body drifted from Go"
    );
}

#[test]
fn every_help_page_is_byte_differential_against_go() {
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repository root")
        .to_path_buf();
    let go = repository.join("build/bin/ptrack");
    if !go.is_file() {
        return;
    }
    let mut paths = vec![
        vec![],
        vec!["help"],
        vec!["completion"],
        vec!["completion", "bash"],
        vec!["completion", "fish"],
        vec!["completion", "powershell"],
        vec!["completion", "zsh"],
        vec!["goal"],
        vec!["summary"],
        vec!["milestone"],
        vec!["plan"],
        vec!["task"],
        vec!["issue"],
        vec!["note"],
        vec!["commit"],
        vec!["hook"],
        vec!["capability"],
    ];
    paths.extend(crate::command::LEAVES.iter().map(|spec| spec.path.to_vec()));

    for path in paths {
        let mut rust_args = vec!["ptrack"];
        rust_args.extend(path.iter().copied());
        rust_args.push("--help");
        let (result, rust_stdout, rust_stderr) = invoke(&rust_args);
        assert_eq!(result.expect("Rust help"), RunOutcome::ExitSuccess);
        assert!(rust_stderr.is_empty());

        let mut go_args = path.clone();
        go_args.push("--help");
        let baseline = std::process::Command::new(&go)
            .args(go_args)
            .output()
            .expect("run Go help baseline");
        assert!(baseline.status.success(), "Go help failed for {path:?}");
        assert_eq!(
            rust_stdout,
            String::from_utf8(baseline.stdout).expect("Go help utf8"),
            "help drift for {path:?}"
        );
    }
}

#[test]
fn implicit_completion_and_help_topics_match_go() {
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repository root")
        .to_path_buf();
    let go = repository.join("build/bin/ptrack");
    if !go.is_file() {
        return;
    }
    for args in [
        ["ptrack", "completion"].as_slice(),
        ["ptrack", "completion", "nope"].as_slice(),
        ["ptrack", "help", "nope"].as_slice(),
        ["ptrack", "help", "task", "nope"].as_slice(),
    ] {
        let (result, rust_stdout, rust_stderr) = invoke(args);
        assert_eq!(result.expect("Rust topic"), RunOutcome::ExitSuccess);
        let baseline = std::process::Command::new(&go)
            .args(&args[1..])
            .output()
            .expect("run Go topic baseline");
        assert!(baseline.status.success());
        assert_eq!(
            rust_stdout.as_bytes(),
            baseline.stdout,
            "stdout for {args:?}"
        );
        assert_eq!(
            rust_stderr.as_bytes(),
            baseline.stderr,
            "stderr for {args:?}"
        );
    }
}
