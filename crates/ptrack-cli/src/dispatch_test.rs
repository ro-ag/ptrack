use std::io::{Read, Write};
use std::path::PathBuf;

use ptrack_app::{
    ActorIdentity, AppError, AppResult, ApplicationPort, CapabilityCancellation,
    CapabilityMcpOutcome, GuideAction, HookAction, HookResult, INVALID_CLAIM_PREFIX,
    INVALID_HOLD_PREFIX, InitRequest, InitResult, Mutation, MutationResult, PlanLifecycleOutcome,
    PlanLifecycleRequest, ProcessOutput,
};
use ptrack_core::{
    Meta, Plan, PlanStatus, ProjectRef, ProjectSnapshot, Task, TaskStatus, Timestamp,
};

use crate::{Io, RunOutcome, run};

struct FakeApplication {
    snapshot: ProjectSnapshot,
    git_output: Option<ProcessOutput>,
    capability_result: Option<AppResult<Vec<u8>>>,
    capability_calls: Vec<(String, String)>,
    mcp_input: Vec<u8>,
    identity: Option<ActorIdentity>,
    claim_owner: Option<&'static str>,
    lifecycle_requests: Vec<PlanLifecycleRequest>,
    lifecycle_results: Vec<AppResult<PlanLifecycleOutcome>>,
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
                    active_plans: Vec::new(),
                    actors: Vec::new(),
                },
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            ),
            git_output: None,
            capability_result: None,
            capability_calls: Vec::new(),
            mcp_input: Vec::new(),
            identity: None,
            claim_owner: None,
            lifecycle_requests: Vec::new(),
            lifecycle_results: Vec::new(),
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
            // Mirrors ProjectStore::set_task_hold / set_plan_hold closely
            // enough to exercise the CLI's success and refusal paths.
            Mutation::SetTaskHold { id, reason } => {
                let task = self
                    .snapshot
                    .tasks
                    .iter_mut()
                    .find(|task| task.id == id)
                    .ok_or(AppError::NotImplemented("test task"))?;
                if reason.is_some() && task.status == TaskStatus::Done {
                    return Err(AppError::Message(format!(
                        "{INVALID_HOLD_PREFIX}task #{id} is done and cannot be put on hold"
                    )));
                }
                task.hold_reason = reason;
            }
            Mutation::SetPlanHold { id, reason } => {
                let plan = self
                    .snapshot
                    .plans
                    .iter_mut()
                    .find(|plan| plan.id == id)
                    .ok_or(AppError::NotImplemented("test plan"))?;
                if reason.is_some() && plan.status != PlanStatus::Active {
                    return Err(AppError::Message(format!(
                        "{INVALID_HOLD_PREFIX}plan #{id} is {} and cannot be put on hold",
                        plan.status.as_str()
                    )));
                }
                plan.hold_reason = reason;
            }
            Mutation::SetActivePlan(id) => self.snapshot.meta.active_plan = id,
            Mutation::StealPlan(_) => self.claim_owner = Some("fake-actor"),
            Mutation::ReleasePlanClaim(id) => {
                if self.claim_owner.is_none() {
                    return Err(AppError::Message(format!(
                        "{INVALID_CLAIM_PREFIX}plan #{id} is not claimed"
                    )));
                }
                self.claim_owner = None;
            }
            _ => return Err(AppError::NotImplemented("test mutation")),
        }
        Ok(MutationResult::None)
    }

    fn plan_lifecycle(&mut self, request: PlanLifecycleRequest) -> AppResult<PlanLifecycleOutcome> {
        self.lifecycle_requests.push(request);
        // `match`, not `unwrap_or_else`: clippy rejects the lazy closure here,
        // and `unwrap_or` would build the fallback error on every call.
        match self.lifecycle_results.pop() {
            Some(result) => result,
            None => Err(AppError::NotImplemented("test plan lifecycle")),
        }
    }

    fn projects(&mut self) -> AppResult<Vec<ProjectRef>> {
        Ok(Vec::new())
    }

    fn identity(&mut self) -> AppResult<Option<ActorIdentity>> {
        Ok(self.identity.clone())
    }

    fn set_identity(&mut self, name: &str) -> AppResult<ActorIdentity> {
        let identity = ActorIdentity {
            id: "00000000000000000000000000".to_owned(),
            name: name.trim().to_owned(),
        };
        self.identity = Some(identity.clone());
        Ok(identity)
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

    fn capability_call(&mut self, tool: &str, arguments: &str) -> AppResult<Vec<u8>> {
        self.capability_calls
            .push((tool.to_owned(), arguments.to_owned()));
        self.capability_result
            .take()
            .unwrap_or(Err(AppError::NotImplemented("test capability")))
    }

    fn capability_mcp(
        &mut self,
        mut input: Box<dyn Read + Send>,
        _output: &mut dyn Write,
        _cancellation: &CapabilityCancellation,
    ) -> AppResult<CapabilityMcpOutcome> {
        input.read_to_end(&mut self.mcp_input)?;
        Ok(CapabilityMcpOutcome::Complete)
    }
}

#[test]
fn capability_call_requires_one_object_forwards_exactly_and_adds_one_raw_newline() {
    let mut application = FakeApplication {
        capability_result: Some(Ok(br#"{"ok":true}"#.to_vec())),
        ..FakeApplication::default()
    };
    let (result, stdout, stderr) = invoke_with(
        &mut application,
        &[
            "ptrack",
            "capability",
            "call",
            "ptrack_http_request",
            "--arguments",
            r#"{"capability_id":1}"#,
        ],
    );
    assert_eq!(result.unwrap(), RunOutcome::ExitSuccess);
    assert_eq!(stdout, "{\"ok\":true}\n");
    assert!(stderr.is_empty());
    assert_eq!(
        application.capability_calls,
        [(
            "ptrack_http_request".to_owned(),
            r#"{"capability_id":1}"#.to_owned()
        )]
    );

    for arguments in ["null", "[]", "1", "true", "{\"x\":1} trailing"] {
        let (error, stdout, stderr) = invoke(&[
            "ptrack",
            "capability",
            "call",
            "ptrack_http_request",
            "--arguments",
            arguments,
        ]);
        assert_eq!(
            error.unwrap_err().to_string(),
            "--arguments must be one JSON object"
        );
        assert!(stdout.is_empty());
        assert!(stderr.is_empty());
    }
}

#[test]
fn capability_mcp_uses_stdin_as_sole_protocol_input_and_emits_no_cli_text() {
    let mut application = FakeApplication::default();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let result = run(
        ["ptrack", "capability", "mcp"].map(str::to_owned),
        &mut application,
        Io {
            stdin: Box::new(std::io::Cursor::new(
                b"{\"jsonrpc\":\"2.0\",\"method\":\"ping\",\"id\":1}\n".to_vec(),
            )),
            stdout: &mut stdout,
            stderr: &mut stderr,
            cancellation: CapabilityCancellation::new(),
        },
    );
    assert_eq!(result.unwrap(), RunOutcome::ExitSuccess);
    assert_eq!(
        application.mcp_input,
        b"{\"jsonrpc\":\"2.0\",\"method\":\"ping\",\"id\":1}\n"
    );
    assert!(stdout.is_empty());
    assert!(stderr.is_empty());
}

/// One active plan with a todo task #1 and a done task #2.
fn seeded() -> FakeApplication {
    let plan = Plan {
        id: 1,
        title: "Build CLI".to_owned(),
        status: PlanStatus::Active,
        milestone_id: 0,
        order: 1,
        created_at: Timestamp::Zero,
        updated_at: Timestamp::Zero,
        hold_reason: None,
        actor: None,
        claim_conflict: false,
        claim_epoch: 0,
        claim_owner: None,
        ulid: None,
    };
    let task = |id: u64, title: &str, status: TaskStatus| Task {
        id,
        plan_id: 1,
        title: title.to_owned(),
        status,
        order: i64::try_from(id).expect("small fixture id fits i64"),
        created_at: Timestamp::Zero,
        updated_at: Timestamp::Zero,
        hold_reason: None,
        actor: None,
        ulid: None,
    };
    FakeApplication {
        snapshot: ProjectSnapshot::new(
            Meta {
                goal: "ship".to_owned(),
                summary: String::new(),
                active_plan: 1,
                created_at: Timestamp::Zero,
                updated_at: Timestamp::Zero,
                format_version: 5,
                last_write_version: "test".to_owned(),
                active_plans: Vec::new(),
                actors: Vec::new(),
            },
            Vec::new(),
            vec![plan],
            vec![
                task(1, "context command", TaskStatus::Todo),
                task(2, "init command", TaskStatus::Done),
            ],
            Vec::new(),
            Vec::new(),
            Vec::new(),
        ),
        ..FakeApplication::default()
    }
}

#[test]
fn task_hold_and_resume_round_trip_through_every_surface() {
    let mut application = seeded();
    let (result, stdout, stderr) = invoke_with(
        &mut application,
        &["ptrack", "task", "hold", "1", "waiting", "on", "review"],
    );
    assert_eq!(result.expect("hold"), RunOutcome::ExitSuccess);
    assert_eq!(stdout, "task #1 on hold: waiting on review\n");
    assert!(stderr.is_empty());

    let (_, stdout, _) = invoke_with(&mut application, &["ptrack", "task", "list"]);
    assert_eq!(
        stdout,
        "#1 [todo] context command (plan 1) [on hold: waiting on review]\n\
         #2 [done] init command (plan 1)\n"
    );

    let (_, stdout, _) = invoke_with(&mut application, &["ptrack", "task", "list", "--json"]);
    assert!(stdout.contains("\"hold_reason\": \"waiting on review\""));
    assert!(stdout.contains("\"hold_reason\": null"));

    let (_, stdout, _) = invoke_with(&mut application, &["ptrack", "task", "show", "1"]);
    assert!(stdout.starts_with("# Task #1 context command [todo] [on hold: waiting on review]\n"));

    let (_, stdout, _) = invoke_with(&mut application, &["ptrack", "status"]);
    assert!(stdout.contains("tasks: 1 todo, 0 doing, 1 done, 0 blocked (1 on hold)\n"));
    let (_, stdout, _) = invoke_with(&mut application, &["ptrack", "status", "--json"]);
    assert!(stdout.contains("\"on_hold\": 1"));

    // A held task is never the next pick even though it is still todo.
    let (_, stdout, _) = invoke_with(&mut application, &["ptrack", "next"]);
    assert_eq!(stdout, "no actionable task in the active plan\n");

    let (result, stdout, _) = invoke_with(&mut application, &["ptrack", "task", "resume", "1"]);
    assert_eq!(result.expect("resume"), RunOutcome::ExitSuccess);
    assert_eq!(stdout, "task #1 resumed\n");
    let (_, stdout, _) = invoke_with(&mut application, &["ptrack", "task", "list"]);
    assert!(!stdout.contains("on hold"));
    let (_, stdout, _) = invoke_with(&mut application, &["ptrack", "next"]);
    assert_eq!(
        stdout,
        "next: [todo] #1 context command (plan: Build CLI)\n"
    );
}

#[test]
fn plan_hold_and_resume_round_trip_through_every_surface() {
    let mut application = seeded();
    let (result, stdout, _) = invoke_with(
        &mut application,
        &["ptrack", "plan", "hold", "1", "budget", "freeze"],
    );
    assert_eq!(result.expect("hold"), RunOutcome::ExitSuccess);
    assert_eq!(stdout, "plan #1 on hold: budget freeze\n");

    let (_, stdout, _) = invoke_with(&mut application, &["ptrack", "plan", "list"]);
    assert_eq!(stdout, "#1 [active] * Build CLI [on hold: budget freeze]\n");

    let (_, stdout, _) = invoke_with(&mut application, &["ptrack", "plan", "list", "--json"]);
    assert!(stdout.contains("\"hold_reason\": \"budget freeze\""));

    let (_, stdout, _) = invoke_with(&mut application, &["ptrack", "plan", "show", "1"]);
    assert!(stdout.starts_with("# Plan #1 Build CLI [active] [on hold: budget freeze]\n"));

    let (_, stdout, _) = invoke_with(&mut application, &["ptrack", "next"]);
    assert_eq!(stdout, "active plan on hold: budget freeze\n");

    // Agents read the reason from a field, not by parsing the message prose.
    let (_, stdout, _) = invoke_with(&mut application, &["ptrack", "next", "--json"]);
    assert_eq!(
        stdout,
        "{\n  \"task\": null,\n  \"plan_title\": \"Build CLI\",\n  \
         \"message\": \"active plan on hold: budget freeze\",\n  \
         \"plan_hold_reason\": \"budget freeze\"\n}\n"
    );

    let (_, stdout, _) = invoke_with(&mut application, &["ptrack", "status"]);
    assert!(stdout.contains("plans: 1 (1 on hold)\n"));
    let (_, stdout, _) = invoke_with(&mut application, &["ptrack", "status", "--json"]);
    assert!(stdout.contains("\"plans_on_hold\": 1"));

    // The digest agrees with `next`: a held plan offers nothing to pick up.
    let (_, stdout, _) = invoke_with(&mut application, &["ptrack", "context"]);
    assert!(stdout.contains(
        "**#1 Build CLI** [on hold: budget freeze]\n\n\
         ### Open tasks\n_plan on hold: budget freeze_\n"
    ));
    assert!(!stdout.contains("- [todo] #1 context command"));

    let (result, stdout, _) = invoke_with(&mut application, &["ptrack", "plan", "resume", "1"]);
    assert_eq!(result.expect("resume"), RunOutcome::ExitSuccess);
    assert_eq!(stdout, "plan #1 resumed\n");
    let (_, stdout, _) = invoke_with(&mut application, &["ptrack", "next"]);
    assert_eq!(
        stdout,
        "next: [todo] #1 context command (plan: Build CLI)\n"
    );
}

#[test]
fn plan_use_claims_a_plan_and_release_gives_it_back() {
    let mut application = seeded();

    // Releasing an unclaimed plan surfaces the store's own sentence, prefix stripped.
    let (result, stdout, stderr) =
        invoke_with(&mut application, &["ptrack", "plan", "release", "1"]);
    assert!(stdout.is_empty());
    assert!(stderr.is_empty());
    assert_eq!(
        result.expect_err("unclaimed").to_string(),
        "plan #1 is not claimed"
    );

    // Plain `plan use` sets the active plan but claims nothing.
    let (result, stdout, _) = invoke_with(&mut application, &["ptrack", "plan", "use", "1"]);
    assert_eq!(result.expect("use"), RunOutcome::ExitSuccess);
    assert!(stdout.is_empty());
    assert_eq!(application.snapshot.meta.active_plan, 1);
    assert!(application.claim_owner.is_none());

    // `--steal` dispatches Mutation::StealPlan and claims the plan.
    let (result, stdout, _) =
        invoke_with(&mut application, &["ptrack", "plan", "use", "1", "--steal"]);
    assert_eq!(result.expect("steal"), RunOutcome::ExitSuccess);
    assert!(stdout.is_empty());
    assert_eq!(application.claim_owner, Some("fake-actor"));

    // Releasing a claimed plan succeeds and prints a confirmation.
    let (result, stdout, _) = invoke_with(&mut application, &["ptrack", "plan", "release", "1"]);
    assert_eq!(result.expect("release"), RunOutcome::ExitSuccess);
    assert_eq!(stdout, "plan #1 released\n");
    assert!(application.claim_owner.is_none());
}

#[test]
fn plan_show_json_carries_null_claim_and_omits_the_name_when_unclaimed() {
    let mut application = seeded();
    let (_, stdout, _) = invoke_with(&mut application, &["ptrack", "plan", "show", "1", "--json"]);
    assert!(stdout.contains("\"claimed_by\": null"));
    assert!(!stdout.contains("claimed_by_name"));
}

#[test]
fn plan_show_json_carries_the_claim_owner_and_its_resolved_name_when_claimed() {
    let mut application = seeded();
    application.snapshot.plans[0].claim_owner = Some("01hzvyekq3s7m8w9x0abcdefgh".to_owned());
    application
        .snapshot
        .meta
        .actors
        .push(("01hzvyekq3s7m8w9x0abcdefgh".to_owned(), "Alice".to_owned()));

    let (_, stdout, _) = invoke_with(&mut application, &["ptrack", "plan", "show", "1", "--json"]);
    assert!(stdout.contains("\"claimed_by\": \"01hzvyekq3s7m8w9x0abcdefgh\""));
    assert!(stdout.contains("\"claimed_by_name\": \"Alice\""));
}

#[test]
fn help_plan_release_renders() {
    let (_, stdout, _) = invoke(&["ptrack", "help", "plan", "release"]);
    assert!(stdout.starts_with("Release your claim on a plan"));
}

#[test]
fn hold_refusals_are_printable_sentences_not_codec_field_paths() {
    let mut application = seeded();
    for (args, expected) in [
        (
            ["ptrack", "task", "hold", "1"].as_slice(),
            "requires at least 2 arg(s), only received 1",
        ),
        (
            ["ptrack", "task", "hold", "1", "   "].as_slice(),
            "the hold reason cannot be blank",
        ),
        (
            ["ptrack", "task", "hold", "1", "a\nb"].as_slice(),
            "the hold reason must be one line without control characters",
        ),
        (
            ["ptrack", "task", "hold", "2", "too late"].as_slice(),
            "task #2 is done and cannot be put on hold",
        ),
        (
            ["ptrack", "plan", "hold", "1"].as_slice(),
            "requires at least 2 arg(s), only received 1",
        ),
    ] {
        let (result, stdout, stderr) = invoke_with(&mut application, args);
        assert!(stdout.is_empty(), "unexpected stdout for {args:?}");
        assert!(stderr.is_empty(), "unexpected stderr for {args:?}");
        assert_eq!(result.expect_err("refused").to_string(), expected);
    }

    // The plan side has its own store refusal, and its own status in the text.
    let mut done_plan = seeded();
    done_plan.snapshot.plans[0].status = PlanStatus::Done;
    let (result, stdout, stderr) =
        invoke_with(&mut done_plan, &["ptrack", "plan", "hold", "1", "later"]);
    assert!(stdout.is_empty());
    assert!(stderr.is_empty());
    assert_eq!(
        result.expect_err("refused").to_string(),
        "plan #1 is done and cannot be put on hold"
    );

    let oversized = "x".repeat(ptrack_core::MAX_HOLD_REASON_BYTES + 1);
    let (result, _, _) = invoke_with(
        &mut application,
        &["ptrack", "task", "hold", "1", &oversized],
    );
    assert_eq!(
        result.expect_err("oversized").to_string(),
        "the hold reason is 1025 bytes; the limit is 1024"
    );
}

#[test]
fn a_hold_reason_is_trimmed_before_it_is_checked_and_stored() {
    let mut application = seeded();
    let (result, stdout, _) = invoke_with(
        &mut application,
        &["ptrack", "task", "hold", "1", "  waiting  "],
    );
    assert_eq!(result.expect("hold"), RunOutcome::ExitSuccess);
    assert_eq!(stdout, "task #1 on hold: waiting\n");

    let (_, stdout, _) = invoke_with(&mut application, &["ptrack", "task", "list"]);
    assert!(stdout.contains("context command (plan 1) [on hold: waiting]\n"));
}

fn invoke(args: &[&str]) -> (Result<RunOutcome, crate::CliError>, String, String) {
    let mut application = FakeApplication::default();
    invoke_with(&mut application, args)
}

fn invoke_with(
    application: &mut FakeApplication,
    args: &[&str],
) -> (Result<RunOutcome, crate::CliError>, String, String) {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let result = run(
        args.iter().map(|value| (*value).to_owned()),
        application,
        Io {
            stdin: Box::new(std::io::empty()),
            stdout: &mut stdout,
            stderr: &mut stderr,
            cancellation: CapabilityCancellation::new(),
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

    // The hold leaves must be registered in help's own child list too, or the
    // leaf help silently falls back to its group.
    let (_, stdout, _) = invoke(&["ptrack", "help", "task", "hold"]);
    assert!(stdout.starts_with("Put a task on hold with a reason"));
    let (_, stdout, _) = invoke(&["ptrack", "help", "plan", "resume"]);
    assert!(stdout.starts_with("Take a plan off hold"));

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
        "{\n  \"goal\": \"goal \\u003cx\\u003e\",\n  \"active_plan\": 0,\n  \"active_plan_title\": \"\",\n  \"plans\": 0,\n  \"todo\": 0,\n  \"doing\": 0,\n  \"done\": 0,\n  \"blocked\": 0,\n  \"on_hold\": 0,\n  \"plans_on_hold\": 0\n}\n"
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
fn config_show_before_set_reports_no_user() {
    let (result, stdout, stderr) = invoke(&["ptrack", "config", "show"]);
    assert_eq!(result.expect("config show"), RunOutcome::ExitSuccess);
    assert_eq!(
        stdout,
        "no user configured (run 'ptrack config set user <name>')\n"
    );
    assert!(stderr.is_empty());
}

#[test]
fn config_set_user_mints_an_identity_and_show_json_reflects_it() {
    let mut application = FakeApplication::default();
    let (result, stdout, stderr) = invoke_with(
        &mut application,
        &["ptrack", "config", "set", "user", "Rodrigo"],
    );
    assert_eq!(result.expect("config set"), RunOutcome::ExitSuccess);
    assert_eq!(stdout, "user Rodrigo (00000000000000000000000000)\n");
    assert!(stderr.is_empty());

    let (result, stdout, stderr) =
        invoke_with(&mut application, &["ptrack", "config", "show", "--json"]);
    assert_eq!(result.expect("config show json"), RunOutcome::ExitSuccess);
    assert_eq!(
        stdout,
        "{\n  \"id\": \"00000000000000000000000000\",\n  \"name\": \"Rodrigo\"\n}\n"
    );
    assert!(stderr.is_empty());
}

#[test]
fn config_set_rejects_an_unknown_key() {
    let (result, stdout, stderr) = invoke(&["ptrack", "config", "set", "badkey", "x"]);
    assert_eq!(
        result.expect_err("unknown config key").to_string(),
        "unknown config key \"badkey\" (want \"user\")"
    );
    assert!(stdout.is_empty());
    assert!(stderr.is_empty());
}
