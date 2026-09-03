use std::io::{Read, Write};
use std::path::PathBuf;

use ptrack_app::{
    ActivityState, ActorIdentity, AgentHandoffInbox, AgentIntelligenceDetail,
    AgentRunObservationV1, AgentRunsV2, AgentRuntimeSummary, AppError, AppResult, ApplicationPort,
    BoundedSnapshot, CapabilityCancellation, CapabilityMcpOutcome, GuideAction, HookAction,
    HookResult, INVALID_CLAIM_PREFIX, INVALID_HOLD_PREFIX, InitRequest, InitResult,
    IntelligenceConfidence, IntelligenceState, LeaseState, Mutation, MutationResult,
    PlanDeleteSummary, PlanLifecycleOutcome, PlanLifecycleRequest, PlanTransferSummary,
    ProcessOutput, ProcessState, RegistrationKind, RelocateRequest, RelocateResult, RunState,
    RuntimeAssociation,
};
use ptrack_core::{
    Commit, MemoryKind, Meta, Note, Plan, PlanStatus, ProjectRef, ProjectSnapshot, Task,
    TaskStatus, Timestamp, would_create_cycle,
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
    relocate_requests: Vec<RelocateRequest>,
    fail_notes: bool,
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
            relocate_requests: Vec::new(),
            fail_notes: false,
        }
    }
}

impl ApplicationPort for FakeApplication {
    fn initialize(&mut self, _request: InitRequest) -> AppResult<InitResult> {
        Err(AppError::NotImplemented("test initialize"))
    }

    fn relocate(&mut self, request: RelocateRequest) -> AppResult<RelocateResult> {
        let root = request
            .root
            .clone()
            .unwrap_or_else(|| PathBuf::from("/cwd/project"));
        self.relocate_requests.push(request);
        Ok(RelocateResult { root })
    }

    fn snapshot(&mut self) -> AppResult<ProjectSnapshot> {
        Ok(self.snapshot.clone())
    }

    // One flat arm per faked mutation, mirroring the real dispatch.
    #[allow(clippy::too_many_lines)]
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
            // Mirrors ProjectStore::add_task_dep / add_plan_dep refusals
            // (unknown id, self-dep, duplicate, cycle) closely enough to
            // exercise the CLI's success and error paths.
            Mutation::AddTaskDep { id, dep_id } => {
                let refused = add_dep_refusal(
                    "task",
                    id,
                    dep_id,
                    &self
                        .snapshot
                        .tasks
                        .iter()
                        .map(|task| (task.id, task.deps.clone()))
                        .collect(),
                );
                if let Some(error) = refused {
                    return Err(error);
                }
                let task = self
                    .snapshot
                    .tasks
                    .iter_mut()
                    .find(|task| task.id == id)
                    .expect("checked above");
                task.deps.push(dep_id);
            }
            Mutation::RemoveTaskDep { id, dep_id } => {
                let task = self
                    .snapshot
                    .tasks
                    .iter_mut()
                    .find(|task| task.id == id)
                    .ok_or_else(|| dep_error(format!("task #{id} does not exist")))?;
                if !task.deps.contains(&dep_id) {
                    return Err(dep_error(format!(
                        "task #{id} does not depend on task #{dep_id}"
                    )));
                }
                task.deps.retain(|&dep| dep != dep_id);
            }
            Mutation::AddPlanDep { id, dep_id } => {
                let refused = add_dep_refusal(
                    "plan",
                    id,
                    dep_id,
                    &self
                        .snapshot
                        .plans
                        .iter()
                        .map(|plan| (plan.id, plan.deps.clone()))
                        .collect(),
                );
                if let Some(error) = refused {
                    return Err(error);
                }
                let plan = self
                    .snapshot
                    .plans
                    .iter_mut()
                    .find(|plan| plan.id == id)
                    .expect("checked above");
                plan.deps.push(dep_id);
            }
            Mutation::RemovePlanDep { id, dep_id } => {
                let plan = self
                    .snapshot
                    .plans
                    .iter_mut()
                    .find(|plan| plan.id == id)
                    .ok_or_else(|| dep_error(format!("plan #{id} does not exist")))?;
                if !plan.deps.contains(&dep_id) {
                    return Err(dep_error(format!(
                        "plan #{id} does not depend on plan #{dep_id}"
                    )));
                }
                plan.deps.retain(|&dep| dep != dep_id);
            }
            // Mirrors the store closely enough to exercise the close/open
            // gates: adds return the created record, status changes mutate in
            // place, notes accumulate in the snapshot.
            Mutation::AddPlan {
                title,
                milestone_id,
            } => {
                let id = self
                    .snapshot
                    .plans
                    .iter()
                    .map(|plan| plan.id)
                    .max()
                    .unwrap_or(0)
                    + 1;
                let plan = Plan {
                    id,
                    title,
                    status: PlanStatus::Active,
                    milestone_id,
                    order: i64::try_from(id).expect("small fixture id fits i64"),
                    created_at: Timestamp::Zero,
                    updated_at: Timestamp::Zero,
                    hold_reason: None,
                    actor: None,
                    claim_conflict: false,
                    claim_epoch: 0,
                    claim_owner: None,
                    ulid: None,
                    deps: Vec::new(),
                };
                self.snapshot.plans.push(plan.clone());
                return Ok(MutationResult::Plan(plan));
            }
            Mutation::AddTask { plan_id, title } => {
                let id = self
                    .snapshot
                    .tasks
                    .iter()
                    .map(|task| task.id)
                    .max()
                    .unwrap_or(0)
                    + 1;
                let task = Task {
                    id,
                    plan_id,
                    title,
                    status: TaskStatus::Todo,
                    order: i64::try_from(id).expect("small fixture id fits i64"),
                    created_at: Timestamp::Zero,
                    updated_at: Timestamp::Zero,
                    hold_reason: None,
                    actor: None,
                    ulid: None,
                    deps: Vec::new(),
                };
                self.snapshot.tasks.push(task.clone());
                return Ok(MutationResult::Task(task));
            }
            Mutation::SetTaskStatus { id, status } => {
                self.snapshot
                    .tasks
                    .iter_mut()
                    .find(|task| task.id == id)
                    .ok_or(AppError::NotImplemented("test task"))?
                    .status = status;
            }
            Mutation::SetPlanStatus { id, status } => {
                self.snapshot
                    .plans
                    .iter_mut()
                    .find(|plan| plan.id == id)
                    .ok_or(AppError::NotImplemented("test plan"))?
                    .status = status;
            }
            Mutation::AddNote {
                target,
                target_id,
                body,
            } => {
                if self.fail_notes {
                    return Err(AppError::Message("test note write failed".to_owned()));
                }
                let id = self
                    .snapshot
                    .notes
                    .iter()
                    .map(|note| note.id)
                    .max()
                    .unwrap_or(0)
                    + 1;
                let note = Note {
                    id,
                    target,
                    target_id,
                    kind: MemoryKind::Legacy,
                    body,
                    created_at: Timestamp::Zero,
                    actor: None,
                    ulid: None,
                };
                self.snapshot.notes.push(note.clone());
                return Ok(MutationResult::Note(note));
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

    fn agent_runs(&mut self) -> AppResult<AgentRunsV2> {
        Ok(AgentRunsV2 {
            generation: 7,
            runs: vec![agent_run()],
            bounds: BoundedSnapshot::new(1, 3),
        })
    }

    fn agent_run(&mut self, run_id: &str) -> AppResult<AgentRunObservationV1> {
        if run_id != "run-safe-123456" {
            return Err(AppError::Message("AgentRun not found".to_owned()));
        }
        Ok(AgentRunObservationV1 {
            generation: 7,
            run: agent_run(),
            intelligence: AgentIntelligenceDetail {
                state: IntelligenceState::Waiting,
                confidence: IntelligenceConfidence::High,
                evidence: Vec::new(),
                event_count: 4,
                last_event_at: None,
            },
            event_bounds: BoundedSnapshot::new(4, 6),
        })
    }

    fn agent_inbox(&mut self) -> AppResult<AgentHandoffInbox> {
        Ok(AgentHandoffInbox {
            items: Vec::new(),
            bounds: BoundedSnapshot::new(0, 0),
            incomplete: true,
        })
    }
}

fn agent_run() -> AgentRuntimeSummary {
    AgentRuntimeSummary {
        run_id: "run-safe-123456".to_owned(),
        registration_kind: RegistrationKind::External,
        terminal_id: String::new(),
        terminal_backed: false,
        terminal_present: false,
        corresponding_terminal: false,
        state: RunState::Running,
        process_state: ProcessState::Unknown,
        lease_state: LeaseState::Active,
        live: true,
        activity_state: ActivityState::Waiting,
        association: Some(RuntimeAssociation {
            plan_id: 26,
            task_id: 209,
            revision: 3,
        }),
        intelligence: None,
    }
}

fn dep_error(detail: impl std::fmt::Display) -> AppError {
    AppError::Message(format!("invalid dependency mutation: {detail}"))
}

/// Store-mirrored refusal checks shared by the task and plan dep fakes.
fn add_dep_refusal(
    kind: &str,
    id: u64,
    dep_id: u64,
    graph: &std::collections::BTreeMap<u64, Vec<u64>>,
) -> Option<AppError> {
    if id == dep_id {
        return Some(dep_error(format!("{kind} #{id} cannot depend on itself")));
    }
    for endpoint in [id, dep_id] {
        if !graph.contains_key(&endpoint) {
            return Some(dep_error(format!("{kind} #{endpoint} does not exist")));
        }
    }
    if graph.get(&id).is_some_and(|deps| deps.contains(&dep_id)) {
        return Some(dep_error(format!(
            "{kind} #{id} already depends on {kind} #{dep_id}"
        )));
    }
    if would_create_cycle(graph, id, dep_id) {
        return Some(dep_error(format!(
            "{kind} #{id} depending on {kind} #{dep_id} would create a dependency cycle"
        )));
    }
    None
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

#[test]
fn project_mcp_is_a_top_level_protocol_only_stdio_command() {
    let mut application = seeded();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let input = concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-11-25\"}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"get_next_task\",\"arguments\":{}}}\n"
    );
    let result = run(
        ["ptrack", "mcp"].map(str::to_owned),
        &mut application,
        Io {
            stdin: Box::new(std::io::Cursor::new(input.as_bytes().to_vec())),
            stdout: &mut stdout,
            stderr: &mut stderr,
            cancellation: CapabilityCancellation::new(),
        },
    );
    assert_eq!(result.unwrap(), RunOutcome::ExitSuccess);
    assert!(stderr.is_empty());
    let rows: Vec<serde_json::Value> = String::from_utf8(stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["result"]["serverInfo"]["name"], "p-track-project");
    assert_eq!(rows[1]["result"]["structuredContent"]["task"]["id"], 1);
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
        deps: Vec::new(),
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
        deps: Vec::new(),
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
    assert!(stdout.starts_with(
        "Goal: ship\n# Task #1 context command [todo] [on hold: waiting on review]\n"
    ));

    let (_, stdout, _) = invoke_with(&mut application, &["ptrack", "status"]);
    assert!(stdout.contains("tasks: 1 todo, 0 doing, 1 done, 0 blocked (1 on hold)\n"));
    let (_, stdout, _) = invoke_with(&mut application, &["ptrack", "status", "--json"]);
    assert!(stdout.contains("\"on_hold\": 1"));

    // A held task is never the next pick even though it is still todo.
    let (_, stdout, _) = invoke_with(&mut application, &["ptrack", "next"]);
    assert_eq!(
        stdout,
        "Goal: ship\nno actionable task in the active plan\n"
    );

    let (result, stdout, _) = invoke_with(&mut application, &["ptrack", "task", "resume", "1"]);
    assert_eq!(result.expect("resume"), RunOutcome::ExitSuccess);
    assert_eq!(stdout, "task #1 resumed\n");
    let (_, stdout, _) = invoke_with(&mut application, &["ptrack", "task", "list"]);
    assert!(!stdout.contains("on hold"));
    let (_, stdout, _) = invoke_with(&mut application, &["ptrack", "next"]);
    assert_eq!(
        stdout,
        "Goal: ship\nnext: [todo] #1 context command (plan: Build CLI)\n"
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
    assert_eq!(stdout, "Goal: ship\nactive plan on hold: budget freeze\n");

    // Agents read the reason from a field, not by parsing the message prose.
    let (_, stdout, _) = invoke_with(&mut application, &["ptrack", "next", "--json"]);
    assert_eq!(
        stdout,
        "{\n  \"goal\": \"ship\",\n  \"task\": null,\n  \"plan_title\": \"Build CLI\",\n  \
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
        "Goal: ship\nnext: [todo] #1 context command (plan: Build CLI)\n"
    );
}

#[test]
fn dep_blocked_tasks_surface_in_next_and_context_on_both_formats() {
    let mut application = seeded();
    // Task #1 (todo) now waits on a fresh open task #3.
    application.snapshot.tasks[0].deps = vec![3];
    application.snapshot.tasks.push(Task {
        id: 3,
        plan_id: 1,
        title: "publish docs".to_owned(),
        status: TaskStatus::Todo,
        order: 3,
        created_at: Timestamp::Zero,
        updated_at: Timestamp::Zero,
        hold_reason: None,
        actor: None,
        ulid: None,
        deps: Vec::new(),
    });

    let (_, stdout, _) = invoke_with(&mut application, &["ptrack", "next"]);
    assert_eq!(
        stdout,
        "Goal: ship\nnext: [todo] #3 publish docs (plan: Build CLI)\nskipped: #1 (waiting on #3)\n"
    );

    let (_, stdout, _) = invoke_with(&mut application, &["ptrack", "next", "--json"]);
    assert!(stdout.contains("\"skipped\""));
    assert!(stdout.contains("\"task_id\": 1"));
    assert!(stdout.contains("\"waiting_on\""));

    let (_, stdout, _) = invoke_with(&mut application, &["ptrack", "context"]);
    assert!(stdout.contains(
        "## Waiting on dependencies (project-wide)\n\
         - #1 context command (plan 1) [waiting on #3]\n"
    ));

    let (_, stdout, _) = invoke_with(&mut application, &["ptrack", "context", "--json"]);
    assert!(stdout.contains("\"waiting_on_deps\""));
    assert!(stdout.contains("\"waiting_on\": [\n        3\n      ]"));
}

#[test]
fn task_dep_add_list_remove_round_trip_on_both_formats() {
    let mut application = seeded();
    let (result, stdout, stderr) = invoke_with(
        &mut application,
        &["ptrack", "task", "dep", "add", "1", "2"],
    );
    assert_eq!(result.expect("add"), RunOutcome::ExitSuccess);
    assert_eq!(stdout, "task #1 depends on task #2\n");
    assert!(stderr.is_empty());

    let (result, stdout, _) =
        invoke_with(&mut application, &["ptrack", "task", "dep", "list", "1"]);
    assert_eq!(result.expect("list"), RunOutcome::ExitSuccess);
    assert_eq!(stdout, "#2 [done] init command (plan 1)\n");

    let (result, stdout, _) = invoke_with(
        &mut application,
        &["ptrack", "task", "dep", "list", "1", "--json"],
    );
    assert_eq!(result.expect("list json"), RunOutcome::ExitSuccess);
    assert_eq!(
        stdout,
        "[\n  {\n    \"id\": 2,\n    \"title\": \"init command\",\n    \"status\": \"done\"\n  }\n]\n"
    );

    let (result, stdout, _) = invoke_with(
        &mut application,
        &["ptrack", "task", "dep", "remove", "1", "2"],
    );
    assert_eq!(result.expect("remove"), RunOutcome::ExitSuccess);
    assert_eq!(stdout, "task #1 no longer depends on task #2\n");

    let (result, stdout, _) =
        invoke_with(&mut application, &["ptrack", "task", "dep", "list", "1"]);
    assert_eq!(result.expect("empty list"), RunOutcome::ExitSuccess);
    assert!(stdout.is_empty());
}

#[test]
fn plan_dep_add_list_remove_round_trip_on_both_formats() {
    let mut application = seeded();
    application.snapshot.plans.push(Plan {
        id: 2,
        title: "Ship docs".to_owned(),
        status: PlanStatus::Done,
        milestone_id: 0,
        order: 2,
        created_at: Timestamp::Zero,
        updated_at: Timestamp::Zero,
        hold_reason: None,
        actor: None,
        claim_conflict: false,
        claim_epoch: 0,
        claim_owner: None,
        ulid: None,
        deps: Vec::new(),
    });

    let (result, stdout, stderr) = invoke_with(
        &mut application,
        &["ptrack", "plan", "dep", "add", "1", "2"],
    );
    assert_eq!(result.expect("add"), RunOutcome::ExitSuccess);
    assert_eq!(stdout, "plan #1 depends on plan #2\n");
    assert!(stderr.is_empty());

    let (result, stdout, _) =
        invoke_with(&mut application, &["ptrack", "plan", "dep", "list", "1"]);
    assert_eq!(result.expect("list"), RunOutcome::ExitSuccess);
    assert_eq!(stdout, "#2 [done] Ship docs\n");

    let (result, stdout, _) = invoke_with(
        &mut application,
        &["ptrack", "plan", "dep", "list", "1", "--json"],
    );
    assert_eq!(result.expect("list json"), RunOutcome::ExitSuccess);
    assert_eq!(
        stdout,
        "[\n  {\n    \"id\": 2,\n    \"title\": \"Ship docs\",\n    \"status\": \"done\"\n  }\n]\n"
    );

    let (result, stdout, _) = invoke_with(
        &mut application,
        &["ptrack", "plan", "dep", "remove", "1", "2"],
    );
    assert_eq!(result.expect("remove"), RunOutcome::ExitSuccess);
    assert_eq!(stdout, "plan #1 no longer depends on plan #2\n");

    let (result, stdout, _) =
        invoke_with(&mut application, &["ptrack", "plan", "dep", "list", "1"]);
    assert_eq!(result.expect("empty list"), RunOutcome::ExitSuccess);
    assert!(stdout.is_empty());
}

#[test]
fn task_dep_refusals_surface_the_store_sentences() {
    let mut application = seeded();
    let (result, _, _) = invoke_with(
        &mut application,
        &["ptrack", "task", "dep", "add", "1", "2"],
    );
    result.expect("seed edge");

    for (args, expected) in [
        (
            ["ptrack", "task", "dep", "add", "2", "1"].as_slice(),
            "invalid dependency mutation: task #2 depending on task #1 would create a dependency cycle",
        ),
        (
            ["ptrack", "task", "dep", "add", "1", "1"].as_slice(),
            "invalid dependency mutation: task #1 cannot depend on itself",
        ),
        (
            ["ptrack", "task", "dep", "add", "1", "99"].as_slice(),
            "invalid dependency mutation: task #99 does not exist",
        ),
        (
            ["ptrack", "task", "dep", "add", "1", "2"].as_slice(),
            "invalid dependency mutation: task #1 already depends on task #2",
        ),
        (
            ["ptrack", "task", "dep", "remove", "1", "99"].as_slice(),
            "invalid dependency mutation: task #1 does not depend on task #99",
        ),
        (
            ["ptrack", "task", "dep", "list", "99"].as_slice(),
            "not found",
        ),
    ] {
        let (result, stdout, stderr) = invoke_with(&mut application, args);
        assert!(stdout.is_empty(), "unexpected stdout for {args:?}");
        assert!(stderr.is_empty(), "unexpected stderr for {args:?}");
        assert_eq!(result.expect_err("refused").to_string(), expected);
    }
}

#[test]
fn plan_dep_refusals_surface_the_store_sentences() {
    let mut application = seeded();
    for (args, expected) in [
        (
            ["ptrack", "plan", "dep", "add", "1", "1"].as_slice(),
            "invalid dependency mutation: plan #1 cannot depend on itself",
        ),
        (
            ["ptrack", "plan", "dep", "add", "1", "99"].as_slice(),
            "invalid dependency mutation: plan #99 does not exist",
        ),
        (
            ["ptrack", "plan", "dep", "remove", "1", "99"].as_slice(),
            "invalid dependency mutation: plan #1 does not depend on plan #99",
        ),
        (
            ["ptrack", "plan", "dep", "list", "99"].as_slice(),
            "not found",
        ),
    ] {
        let (result, stdout, stderr) = invoke_with(&mut application, args);
        assert!(stdout.is_empty(), "unexpected stdout for {args:?}");
        assert!(stderr.is_empty(), "unexpected stderr for {args:?}");
        assert_eq!(result.expect_err("refused").to_string(), expected);
    }
}

#[test]
fn dep_help_and_arg_count_errors_are_coherent() {
    let (result, stdout, _) = invoke(&["ptrack", "task", "dep", "--help"]);
    assert_eq!(result.expect("group help"), RunOutcome::ExitSuccess);
    assert!(stdout.starts_with("Manage task dependency edges"));
    assert!(stdout.contains("  ptrack task dep [command]"));
    assert!(stdout.contains("  add"));
    assert!(stdout.contains("  list"));
    assert!(stdout.contains("  remove"));

    let (_, stdout, _) = invoke(&["ptrack", "help", "plan", "dep", "add"]);
    assert!(stdout.starts_with("Add a dependency edge"));
    assert!(stdout.contains("  ptrack plan dep add <id> <dep-id> [flags]"));

    let (result, _, _) = invoke(&["ptrack", "task", "dep", "add", "1"]);
    assert_eq!(
        result.expect_err("arity").to_string(),
        "accepts 2 arg(s), received 1"
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
fn plan_delete_without_force_prints_preview_and_refuses() {
    let mut application = FakeApplication::default();
    application
        .lifecycle_results
        .push(Ok(PlanLifecycleOutcome::Preview(PlanDeleteSummary {
            plan_id: 3,
            title: "Doomed".to_owned(),
            tasks: 2,
            notes: 1,
            commits_unlinked: 4,
            issues: vec![(7, "crash on save".to_owned())],
        })));
    let (result, stdout, _stderr) =
        invoke_with(&mut application, &["ptrack", "plan", "delete", "3"]);
    assert_eq!(
        application.lifecycle_requests,
        vec![PlanLifecycleRequest::DeletePreview { plan_id: 3 }]
    );
    assert!(
        stdout.contains(
            "plan #3 \"Doomed\": 2 task(s), 1 note(s), 1 issue link(s), 4 commit record(s)"
        )
    );
    assert!(stdout.contains("would detach issue #7 \"crash on save\""));
    assert!(result.unwrap_err().to_string().contains("--force"));
}

#[test]
fn plan_delete_with_force_deletes_and_prints_the_same_summary() {
    let mut application = FakeApplication::default();
    application
        .lifecycle_results
        .push(Ok(PlanLifecycleOutcome::Deleted(PlanDeleteSummary {
            plan_id: 3,
            title: "Doomed".to_owned(),
            tasks: 2,
            notes: 1,
            commits_unlinked: 0,
            issues: vec![(7, "crash on save".to_owned())],
        })));
    let (result, stdout, _stderr) = invoke_with(
        &mut application,
        &["ptrack", "plan", "delete", "3", "--force"],
    );
    result.unwrap();
    assert_eq!(
        application.lifecycle_requests,
        vec![PlanLifecycleRequest::Delete { plan_id: 3 }]
    );
    assert!(stdout.contains("detached issue #7 \"crash on save\""));
    assert!(stdout.contains("plan #3 deleted"));
}

#[test]
fn plan_move_requires_to_and_prints_both_projects_and_the_new_id() {
    let mut application = FakeApplication::default();
    let (missing, _stdout, _stderr) =
        invoke_with(&mut application, &["ptrack", "plan", "move", "3"]);
    assert!(missing.unwrap_err().to_string().contains("--to"));
    assert!(application.lifecycle_requests.is_empty());

    application
        .lifecycle_results
        .push(Ok(PlanLifecycleOutcome::Transferred(PlanTransferSummary {
            source_plan_id: 3,
            new_plan_id: 9,
            title: "Landed".to_owned(),
            source_project: "alpha".to_owned(),
            target_project: "beta".to_owned(),
            moved: true,
            tasks: 2,
            notes: 1,
            issues: 1,
            commits: 4,
        })));
    let (result, stdout, _stderr) = invoke_with(
        &mut application,
        &[
            "ptrack", "plan", "move", "3", "--to", "beta", "--as", "Landed",
        ],
    );
    result.unwrap();
    assert_eq!(
        application.lifecycle_requests,
        vec![PlanLifecycleRequest::Move {
            plan_id: 3,
            to: "beta".to_owned(),
            rename: Some("Landed".to_owned()),
        }]
    );
    assert!(stdout.contains(
        "moved plan #3 from alpha to beta: now plan #9 \"Landed\" (2 tasks, 1 notes, 1 issues, 4 commits carried from source)"
    ));
}

#[test]
fn plan_copy_passes_optional_target_and_rename_through() {
    let mut application = FakeApplication::default();
    application
        .lifecycle_results
        .push(Ok(PlanLifecycleOutcome::Transferred(PlanTransferSummary {
            source_plan_id: 3,
            new_plan_id: 12,
            title: "Second".to_owned(),
            source_project: "alpha".to_owned(),
            target_project: "alpha".to_owned(),
            moved: false,
            tasks: 0,
            notes: 0,
            issues: 0,
            commits: 0,
        })));
    let (result, stdout, _stderr) = invoke_with(
        &mut application,
        &["ptrack", "plan", "copy", "3", "--as", "Second"],
    );
    result.unwrap();
    assert_eq!(
        application.lifecycle_requests,
        vec![PlanLifecycleRequest::Copy {
            plan_id: 3,
            to: None,
            rename: Some("Second".to_owned()),
        }]
    );
    assert!(stdout.contains("copied plan #3 to alpha: new plan #12 \"Second\""));
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
fn agent_commands_render_safe_text_json_and_complete_help() {
    let (result, stdout, _) = invoke(&["ptrack", "agent", "list"]);
    assert_eq!(result.unwrap(), RunOutcome::ExitSuccess);
    assert!(stdout.contains("run-safe-123456\twaiting\texternal\tlive\tplan #26 / task #209"));
    assert!(stdout.contains("… 2 more registered run(s)"));

    let (result, stdout, _) = invoke(&["ptrack", "agent", "show", "run-safe-123456", "--json"]);
    assert_eq!(result.unwrap(), RunOutcome::ExitSuccess);
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(json["intelligence"]["state"], "waiting");
    assert!(json.get("pid").is_none());
    assert!(!stdout.contains("cwd"));
    assert!(!stdout.contains("provider"));

    let (result, stdout, _) = invoke(&["ptrack", "agent", "inbox"]);
    assert_eq!(result.unwrap(), RunOutcome::ExitSuccess);
    assert!(stdout.contains("proposal only"));
    assert!(stdout.contains("no pending agent handoffs"));
    assert!(stdout.contains("analysis incomplete"));

    let (_, stdout, _) = invoke(&["ptrack", "help", "agent"]);
    for leaf in ["inbox", "list", "show"] {
        assert!(stdout.contains(leaf), "missing agent help leaf {leaf}");
    }
    let (_, stdout, _) = invoke(&["ptrack", "agent", "show", "--help"]);
    assert!(stdout.contains("agent show <run-id>"));
    assert!(stdout.contains("--json"));
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

#[test]
fn relocate_routes_the_root_flag_and_prints_the_new_registration() {
    let mut application = FakeApplication::default();
    let (result, stdout, stderr) = invoke_with(
        &mut application,
        &["ptrack", "relocate", "--root", "/moved/project"],
    );
    assert_eq!(result.unwrap(), RunOutcome::ExitSuccess);
    assert_eq!(stdout, "project re-registered at /moved/project\n");
    assert!(stderr.is_empty());
    assert_eq!(
        application.relocate_requests,
        [RelocateRequest {
            root: Some(PathBuf::from("/moved/project")),
        }]
    );

    let mut application = FakeApplication::default();
    let (result, stdout, _) = invoke_with(&mut application, &["ptrack", "relocate"]);
    assert_eq!(result.unwrap(), RunOutcome::ExitSuccess);
    assert_eq!(stdout, "project re-registered at /cwd/project\n");
    assert_eq!(application.relocate_requests, [RelocateRequest::default()]);
}

fn commit_for_task(id: u64, task_id: u64) -> Commit {
    Commit {
        id,
        sha: format!("sha{id}"),
        subject: "wire it in".to_owned(),
        plan_id: 1,
        task_id,
        created_at: Timestamp::Zero,
        actor: None,
        ulid: None,
    }
}

#[test]
fn task_done_requires_summary_and_linked_commit() {
    let mut application = seeded();
    let (result, _, _) = invoke_with(&mut application, &["ptrack", "task", "done", "1"]);
    let message = result.expect_err("gated").to_string();
    assert!(message.contains("--summary"), "{message}");
    assert!(message.contains("no commit is linked"), "{message}");
    assert_eq!(application.snapshot.tasks[0].status, TaskStatus::Todo);

    // Summary alone is not enough while no commit is linked.
    let (result, _, _) = invoke_with(
        &mut application,
        &["ptrack", "task", "done", "1", "--summary", "wired into CLI"],
    );
    assert!(result.is_err());

    application.snapshot.commits.push(commit_for_task(1, 1));
    let (result, stdout, _) = invoke_with(
        &mut application,
        &["ptrack", "task", "done", "1", "--summary", "wired into CLI"],
    );
    assert_eq!(result.expect("close"), RunOutcome::ExitSuccess);
    assert_eq!(stdout, "Linked commits: 1\n");
    assert_eq!(application.snapshot.tasks[0].status, TaskStatus::Done);
    assert!(
        application
            .snapshot
            .notes
            .iter()
            .any(|note| note.target_id == 1 && note.body == "closeout: wired into CLI")
    );
}

#[test]
fn task_done_keeps_the_task_open_when_required_evidence_cannot_be_written() {
    let mut application = seeded();
    application.snapshot.commits.push(commit_for_task(1, 1));
    application.fail_notes = true;

    let (result, _, _) = invoke_with(
        &mut application,
        &["ptrack", "task", "done", "1", "--summary", "wired into CLI"],
    );

    assert_eq!(result.unwrap_err().to_string(), "test note write failed");
    assert_eq!(application.snapshot.tasks[0].status, TaskStatus::Todo);
}

#[test]
fn task_done_force_bypasses_the_gate_and_records_the_override() {
    let mut application = seeded();
    let (result, stdout, _) = invoke_with(
        &mut application,
        &["ptrack", "task", "done", "1", "--force"],
    );
    assert_eq!(result.expect("forced close"), RunOutcome::ExitSuccess);
    assert_eq!(stdout, "Linked commits: 0\n");
    assert_eq!(application.snapshot.tasks[0].status, TaskStatus::Done);
    assert!(
        application.snapshot.notes.iter().any(
            |note| note.target_id == 1 && note.body.starts_with("override: closed via --force")
        )
    );
}

#[test]
fn a_started_task_blocks_opening_new_work_until_finished_or_parked() {
    let mut application = seeded();
    application.snapshot.tasks[0].status = TaskStatus::Doing;

    for args in [
        &["ptrack", "task", "add", "new work"][..],
        &["ptrack", "task", "start", "2"][..],
        &["ptrack", "plan", "add", "next plan"][..],
    ] {
        let (result, _, _) = invoke_with(&mut application, args);
        let message = result.expect_err("wip gate").to_string();
        assert!(message.contains("task #1"), "{message}");
        assert!(message.contains("--force"), "{message}");
    }

    // Parking the started task reopens the gate.
    let (result, _, _) = invoke_with(
        &mut application,
        &["ptrack", "task", "hold", "1", "waiting", "on", "review"],
    );
    assert_eq!(result.expect("hold"), RunOutcome::ExitSuccess);
    application.snapshot.tasks[0].status = TaskStatus::Blocked;
    let (result, _, _) = invoke_with(&mut application, &["ptrack", "task", "add", "new work"]);
    assert_eq!(result.expect("add"), RunOutcome::ExitSuccess);
}

#[test]
fn wip_gate_force_records_the_override_on_the_new_item() {
    let mut application = seeded();
    application.snapshot.tasks[0].status = TaskStatus::Doing;
    let (result, stdout, _) = invoke_with(
        &mut application,
        &["ptrack", "task", "add", "new work", "--force"],
    );
    assert_eq!(result.expect("forced add"), RunOutcome::ExitSuccess);
    assert!(stdout.contains("task #3 new work"));
    assert!(
        application
            .snapshot
            .notes
            .iter()
            .any(|note| note.target_id == 3 && note.body.contains("--force while task #1"))
    );
}

#[test]
fn wip_gate_scopes_to_the_configured_identity() {
    let mut application = seeded();
    application.snapshot.tasks[0].status = TaskStatus::Doing;
    application.snapshot.tasks[0].actor = Some("someone-else".to_owned());
    application.identity = Some(ActorIdentity {
        id: "me".to_owned(),
        name: "Me".to_owned(),
    });

    // Another agent's in-progress work never blocks this identity.
    let (result, _, _) = invoke_with(&mut application, &["ptrack", "task", "add", "my work"]);
    assert_eq!(result.expect("add"), RunOutcome::ExitSuccess);

    // The caller's own started task does.
    application.snapshot.tasks[0].actor = Some("me".to_owned());
    let (result, _, _) = invoke_with(&mut application, &["ptrack", "task", "add", "more work"]);
    assert!(result.is_err());
}

#[test]
fn plan_done_blocks_open_tasks_then_prints_the_checkpoint() {
    let mut application = seeded();
    let (result, _, _) = invoke_with(&mut application, &["ptrack", "plan", "done", "1"]);
    let message = result.expect_err("open tasks").to_string();
    assert!(message.contains("open tasks remain (#1)"), "{message}");
    assert_eq!(application.snapshot.plans[0].status, PlanStatus::Active);

    application.snapshot.tasks[0].status = TaskStatus::Done;
    let (result, stdout, _) = invoke_with(&mut application, &["ptrack", "plan", "done", "1"]);
    assert_eq!(result.expect("close"), RunOutcome::ExitSuccess);
    assert_eq!(application.snapshot.plans[0].status, PlanStatus::Done);
    assert!(stdout.starts_with("Plan #1 done.\n"), "{stdout}");
    assert!(stdout.contains("Goal: ship"), "{stdout}");
    assert!(stdout.contains("Remaining open plans: none"), "{stdout}");
    assert!(
        stdout.contains("CHECKPOINT — before continuing"),
        "{stdout}"
    );
}

#[test]
fn plan_done_force_closes_over_open_tasks_and_records_the_override() {
    let mut application = seeded();
    let (result, stdout, _) = invoke_with(
        &mut application,
        &["ptrack", "plan", "done", "1", "--force"],
    );
    assert_eq!(result.expect("forced close"), RunOutcome::ExitSuccess);
    assert_eq!(application.snapshot.plans[0].status, PlanStatus::Done);
    assert!(stdout.contains("CHECKPOINT"), "{stdout}");
    assert!(
        application
            .snapshot
            .notes
            .iter()
            .any(|note| note.target_id == 1
                && note.body == "override: closed via --force with open tasks #1")
    );
}

#[test]
fn plan_add_appends_the_integration_task_unless_skipped() {
    let mut application = seeded();
    let (result, stdout, _) = invoke_with(&mut application, &["ptrack", "plan", "add", "Storage"]);
    assert_eq!(result.expect("add"), RunOutcome::ExitSuccess);
    assert!(stdout.contains("plan #2 Storage"), "{stdout}");
    assert!(
        stdout.contains("task #3 Integrate and verify against goal: ship (plan 2)"),
        "{stdout}"
    );

    let (result, stdout, _) = invoke_with(
        &mut application,
        &["ptrack", "plan", "add", "API", "--no-verify-task"],
    );
    assert_eq!(result.expect("add"), RunOutcome::ExitSuccess);
    assert!(!stdout.contains("Integrate and verify"), "{stdout}");
}

#[test]
fn checkpoint_renders_the_whole_picture_on_both_formats() {
    let mut application = seeded();
    let (result, stdout, _) = invoke_with(&mut application, &["ptrack", "checkpoint"]);
    assert_eq!(result.expect("checkpoint"), RunOutcome::ExitSuccess);
    assert!(stdout.starts_with("Goal: ship\n"), "{stdout}");
    assert!(
        stdout.contains("Remaining open plans: #1 Build CLI"),
        "{stdout}"
    );
    assert!(stdout.contains("Open issues: 0 (0 high)"), "{stdout}");
    assert!(
        stdout.contains("CHECKPOINT — before continuing"),
        "{stdout}"
    );

    let (_, stdout, _) = invoke_with(&mut application, &["ptrack", "checkpoint", "--json"]);
    assert!(stdout.contains("\"goal\": \"ship\""), "{stdout}");
    assert!(stdout.contains("\"open_plans\""), "{stdout}");
}
