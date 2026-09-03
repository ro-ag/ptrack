use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use ptrack_capability::McpCancellation;
use ptrack_core::{NoteTarget, TaskStatus};
use ptrack_store::{ActiveBinding, GlobalStore, ProjectStore, StoreKind};
use serde_json::{Value, json};

use crate::{
    ApplicationPort, LocalApplication, Mutation, MutationResult, ProjectEndpoint,
    WorkspaceBindings, serve_project_mcp,
};

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(name: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "ptrack-project-mcp-{name}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("create test directory");
        ptrack_store::protect_private_directory(&path).expect("protect test directory");
        Self(std::fs::canonicalize(path).expect("canonical test directory"))
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn binding(path: &Path, kind: StoreKind, id: &str) -> ActiveBinding {
    ActiveBinding {
        generation: 9,
        database_id: id.to_owned(),
        kind,
        canonical_path: path.to_path_buf(),
    }
}

fn configured(test: &TestDirectory) -> LocalApplication {
    let root = test.0.join("project");
    let home = test.0.join("home");
    std::fs::create_dir_all(root.join(".ptrack")).expect("project directory");
    ptrack_store::protect_private_directory(&root.join(".ptrack"))
        .expect("protect project directory");
    std::fs::create_dir_all(&home).expect("home directory");
    let project_database = root.join(".ptrack/ptrack.redb");
    let global_database = home.join("global.redb");
    let project_binding = binding(&project_database, StoreKind::Project, "project-9");
    let global_binding = binding(&global_database, StoreKind::Global, "global-9");
    drop(
        GlobalStore::create_new(&global_database, global_binding.clone())
            .expect("create global store"),
    );
    drop(
        ProjectStore::create_new(&project_database, project_binding.clone(), "test")
            .expect("create project store"),
    );
    LocalApplication::new(WorkspaceBindings {
        current_dir: root.clone(),
        project: Some(ProjectEndpoint {
            root,
            database: project_database,
            binding: project_binding,
        }),
        global_database,
        global_binding,
        global_home: home,
        writer_version: "test".to_owned(),
    })
}

fn seed(application: &mut LocalApplication, link_commit: bool) -> (u64, u64) {
    application
        .mutate(Mutation::SetGoal("ship MCP parity".to_owned()))
        .unwrap();
    let MutationResult::Plan(plan) = application
        .mutate(Mutation::AddPlan {
            title: "MCP".to_owned(),
            milestone_id: 0,
        })
        .unwrap()
    else {
        panic!("plan result");
    };
    application
        .mutate(Mutation::SetActivePlan(plan.id))
        .unwrap();
    let MutationResult::Task(task) = application
        .mutate(Mutation::AddTask {
            plan_id: plan.id,
            title: "serve tools".to_owned(),
        })
        .unwrap()
    else {
        panic!("task result");
    };
    if link_commit {
        application
            .mutate(Mutation::AddCommit {
                sha: "abc123".to_owned(),
                subject: "wire MCP".to_owned(),
                plan_id: plan.id,
                task_id: task.id,
            })
            .unwrap();
    }
    (plan.id, task.id)
}

fn serve(application: &mut LocalApplication, input: String) -> Vec<Value> {
    let mut output = Vec::new();
    serve_project_mcp(
        application,
        Box::new(std::io::Cursor::new(input.into_bytes())),
        &mut output,
        &McpCancellation::new(),
    )
    .unwrap();
    String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn request_lines(requests: &[Value]) -> String {
    let mut input = requests
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    input.push('\n');
    input
}

#[test]
fn project_mcp_lists_and_executes_the_four_bounded_structured_tools() {
    let directory = TestDirectory::new("round-trip");
    let mut application = configured(&directory);
    let (_, task_id) = seed(&mut application, true);
    let input = request_lines(&[
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-11-25"}}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "get_context", "arguments": {}}}),
        json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "get_next_task", "arguments": {}}}),
        json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "add_note", "arguments": {"target": "task", "target_id": task_id, "body": "decision"}}}),
        json!({"jsonrpc": "2.0", "id": 6, "method": "tools/call", "params": {"name": "complete_task", "arguments": {"task_id": task_id, "summary": "wired into MCP"}}}),
    ]);
    let rows = serve(&mut application, input);

    assert_eq!(rows[0]["result"]["serverInfo"]["name"], "p-track-project");
    let tools = rows[1]["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 4);
    assert!(tools.iter().all(|tool| {
        tool["inputSchema"]["additionalProperties"] == false
            && tool["annotations"]["openWorldHint"] == false
    }));
    assert_eq!(
        rows[2]["result"]["structuredContent"]["active_plan"]["id"],
        1
    );
    assert_eq!(
        rows[3]["result"]["structuredContent"]["task"]["id"],
        task_id
    );
    assert_eq!(rows[4]["result"]["structuredContent"]["target"], "task");
    assert_eq!(rows[5]["result"]["structuredContent"]["linked_commits"], 1);

    let snapshot = application.snapshot().unwrap();
    assert_eq!(snapshot.task(task_id).unwrap().status, TaskStatus::Done);
    assert!(snapshot.notes.iter().any(|note| {
        note.target == NoteTarget::Task
            && note.target_id == task_id
            && note.body == "closeout: wired into MCP"
    }));
}

#[test]
fn project_mcp_rejects_orphan_notes_and_a_completion_without_evidence() {
    let directory = TestDirectory::new("refusals");
    let mut application = configured(&directory);
    let (_, task_id) = seed(&mut application, false);
    let input = request_lines(&[
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-11-25"}}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "add_note", "arguments": {"target": "task", "target_id": 999, "body": "orphan"}}}),
        json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "complete_task", "arguments": {"task_id": task_id}}}),
        json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "complete_task", "arguments": {"task_id": 0, "summary": "invalid"}}}),
    ]);
    let rows = serve(&mut application, input);
    assert_eq!(rows[1]["result"]["isError"], true);
    assert_eq!(
        rows[1]["result"]["content"][0]["text"],
        "task #999 not found"
    );
    assert_eq!(rows[2]["result"]["isError"], true);
    assert!(
        rows[2]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("cannot close task")
    );
    assert_eq!(rows[3]["result"]["isError"], true);
    assert_eq!(
        rows[3]["result"]["content"][0]["text"],
        "task_id must be positive"
    );
    assert_eq!(
        application
            .snapshot()
            .unwrap()
            .task(task_id)
            .unwrap()
            .status,
        TaskStatus::Todo
    );
}
