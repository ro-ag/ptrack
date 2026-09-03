use std::io::{Read, Write};

use ptrack_capability::{
    McpCancellation, McpServeOutcome, ToolCall, ToolDefinition, serve_mcp_with_tools,
};
use ptrack_core::{Digest, NextView, NoteTarget, TaskLine, context, next};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    AppError, AppResult, ApplicationPort, CapabilityMcpOutcome, Mutation, MutationResult,
    complete_task,
};

const TOOL_GET_CONTEXT: &str = "get_context";
const TOOL_GET_NEXT_TASK: &str = "get_next_task";
const TOOL_COMPLETE_TASK: &str = "complete_task";
const TOOL_ADD_NOTE: &str = "add_note";
const MAX_TOOL_TEXT_CHARS: usize = 65_536;

/// Serves the project planning MCP surface over newline-delimited stdio.
///
/// # Errors
/// Returns project access, framing, output, or tool-dispatch errors. Individual
/// tool failures are encoded as MCP tool results by the transport.
pub fn serve_project_mcp(
    application: &mut dyn ApplicationPort,
    input: Box<dyn Read + Send>,
    output: &mut dyn Write,
    cancellation: &McpCancellation,
) -> AppResult<CapabilityMcpOutcome> {
    let tools = project_tool_definitions();
    let outcome = serve_mcp_with_tools(
        input,
        output,
        cancellation,
        "p-track-project",
        "1",
        &tools,
        |_, call| dispatch_tool(application, call),
    )
    .map_err(|error| AppError::Message(error.to_string()))?;
    Ok(match outcome {
        McpServeOutcome::Complete => CapabilityMcpOutcome::Complete,
        McpServeOutcome::Cancelled => CapabilityMcpOutcome::Cancelled,
    })
}

fn project_tool_definitions() -> Vec<ToolDefinition> {
    let read_annotations = json!({
        "readOnlyHint": true,
        "destructiveHint": false,
        "idempotentHint": true,
        "openWorldHint": false
    });
    let write_annotations = json!({
        "readOnlyHint": false,
        "destructiveHint": false,
        "idempotentHint": false,
        "openWorldHint": false
    });
    let text = json!({
        "type": "string",
        "minLength": 1,
        "maxLength": MAX_TOOL_TEXT_CHARS
    });
    vec![
        ToolDefinition {
            name: TOOL_GET_CONTEXT.to_owned(),
            title: "Get p-track context".to_owned(),
            description: "Get the bounded project resume digest".to_owned(),
            input_schema: empty_object_schema(),
            annotations: read_annotations.clone(),
        },
        ToolDefinition {
            name: TOOL_GET_NEXT_TASK.to_owned(),
            title: "Get next p-track task".to_owned(),
            description: "Get the next actionable task in the active plan".to_owned(),
            input_schema: empty_object_schema(),
            annotations: read_annotations,
        },
        ToolDefinition {
            name: TOOL_COMPLETE_TASK.to_owned(),
            title: "Complete p-track task".to_owned(),
            description: "Complete a task with summary and linked-commit evidence".to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "task_id": {"type": "integer", "minimum": 1},
                    "summary": text.clone(),
                    "force": {"type": "boolean"}
                },
                "additionalProperties": false,
                "required": ["task_id"]
            }),
            annotations: json!({
                "readOnlyHint": false,
                "destructiveHint": true,
                "idempotentHint": false,
                "openWorldHint": false
            }),
        },
        ToolDefinition {
            name: TOOL_ADD_NOTE.to_owned(),
            title: "Add p-track note".to_owned(),
            description: "Add a project, plan, or task note".to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "target": {"type": "string", "enum": ["project", "plan", "task"]},
                    "target_id": {"type": "integer", "minimum": 1},
                    "body": text
                },
                "additionalProperties": false,
                "required": ["target", "body"]
            }),
            annotations: write_annotations,
        },
    ]
}

fn empty_object_schema() -> Value {
    json!({
        "type": "object",
        "properties": {},
        "additionalProperties": false
    })
}

fn dispatch_tool(application: &mut dyn ApplicationPort, call: ToolCall) -> AppResult<Value> {
    match call.name.as_str() {
        TOOL_GET_CONTEXT => {
            decode_arguments::<EmptyArguments>(call.arguments, TOOL_GET_CONTEXT)?;
            Ok(context_value(&context(&application.snapshot()?)))
        }
        TOOL_GET_NEXT_TASK => {
            decode_arguments::<EmptyArguments>(call.arguments, TOOL_GET_NEXT_TASK)?;
            Ok(next_value(
                &next(&application.snapshot()?)
                    .map_err(|error| AppError::Message(error.to_string()))?,
            ))
        }
        TOOL_COMPLETE_TASK => {
            let arguments =
                decode_arguments::<CompleteTaskArguments>(call.arguments, TOOL_COMPLETE_TASK)?;
            if arguments.task_id == 0 {
                return Err(AppError::Message("task_id must be positive".to_owned()));
            }
            bounded_optional_text(arguments.summary.as_deref(), "summary")?;
            let result = complete_task(
                application,
                arguments.task_id,
                arguments.summary,
                arguments.force,
            )?;
            Ok(json!({
                "task_id": result.task_id,
                "status": "done",
                "linked_commits": result.linked_commits,
                "closeout_note_id": result.closeout_note.map(|note| note.id),
                "override_note_id": result.override_note.map(|note| note.id)
            }))
        }
        TOOL_ADD_NOTE => {
            let arguments = decode_arguments::<AddNoteArguments>(call.arguments, TOOL_ADD_NOTE)?;
            let body = bounded_required_text(&arguments.body, "note body")?;
            let snapshot = application.snapshot()?;
            let (target, target_id) = match arguments.target.as_str() {
                "project" if arguments.target_id.is_none() => (NoteTarget::Project, 0),
                "project" => {
                    return Err(AppError::Message(
                        "target_id must be omitted for a project note".to_owned(),
                    ));
                }
                "plan" => {
                    let target_id = required_target_id(arguments.target_id, "plan")?;
                    if snapshot.plan(target_id).is_none() {
                        return Err(AppError::Message(format!("plan #{target_id} not found")));
                    }
                    (NoteTarget::Plan, target_id)
                }
                "task" => {
                    let target_id = required_target_id(arguments.target_id, "task")?;
                    if snapshot.task(target_id).is_none() {
                        return Err(AppError::Message(format!("task #{target_id} not found")));
                    }
                    (NoteTarget::Task, target_id)
                }
                _ => {
                    return Err(AppError::Message(
                        "note target must be project, plan, or task".to_owned(),
                    ));
                }
            };
            let result = application.mutate(Mutation::AddNote {
                target,
                target_id,
                body,
            })?;
            let MutationResult::Note(note) = result else {
                return Err(AppError::Message(
                    "internal mutation result mismatch".to_owned(),
                ));
            };
            Ok(json!({
                "id": note.id,
                "target": note.target.as_str(),
                "target_id": note.target_id,
                "body": note.body
            }))
        }
        _ => Err(AppError::Message("unknown project MCP tool".to_owned())),
    }
}

fn decode_arguments<T: for<'de> Deserialize<'de>>(arguments: Value, tool: &str) -> AppResult<T> {
    serde_json::from_value(arguments)
        .map_err(|_| AppError::Message(format!("invalid arguments for {tool}")))
}

fn bounded_optional_text(value: Option<&str>, label: &str) -> AppResult<()> {
    if value.is_some_and(|value| value.chars().count() > MAX_TOOL_TEXT_CHARS) {
        return Err(AppError::Message(format!(
            "{label} exceeds the {MAX_TOOL_TEXT_CHARS} character limit"
        )));
    }
    Ok(())
}

fn bounded_required_text(value: &str, label: &str) -> AppResult<String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(AppError::Message(format!("{label} is required")));
    }
    bounded_optional_text(Some(value), label)?;
    Ok(value.to_owned())
}

fn required_target_id(target_id: Option<u64>, target: &str) -> AppResult<u64> {
    target_id
        .filter(|id| *id > 0)
        .ok_or_else(|| AppError::Message(format!("target_id is required for a {target} note")))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyArguments {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CompleteTaskArguments {
    task_id: u64,
    summary: Option<String>,
    #[serde(default)]
    force: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AddNoteArguments {
    target: String,
    target_id: Option<u64>,
    body: String,
}

fn task_line_value(task: &TaskLine) -> Value {
    json!({
        "id": task.id,
        "plan_id": task.plan_id,
        "title": task.title,
        "status": task.status,
        "hold_reason": task.hold_reason
    })
}

fn context_value(digest: &Digest) -> Value {
    let active_plan = digest.active_plan.as_ref().map(|plan| {
        json!({
            "id": plan.id,
            "title": plan.title,
            "open_tasks": plan.open_tasks.iter().map(task_line_value).collect::<Vec<_>>(),
            "hold_reason": plan.hold_reason,
            "waiting_on": plan.waiting_on
        })
    });
    json!({
        "goal": digest.goal,
        "summary": digest.summary,
        "active_plan": active_plan,
        "blocked": digest.blocked.iter().map(task_line_value).collect::<Vec<_>>(),
        "blocked_more": digest.blocked_more,
        "on_hold": digest.on_hold.iter().map(task_line_value).collect::<Vec<_>>(),
        "on_hold_more": digest.on_hold_more,
        "waiting_on_deps": digest.waiting_on_deps.iter().map(|entry| json!({
            "task": task_line_value(&entry.task),
            "waiting_on": entry.waiting_on
        })).collect::<Vec<_>>(),
        "waiting_on_deps_more": digest.waiting_on_deps_more,
        "open_issues": digest.open_issues.iter().map(|issue| json!({
            "id": issue.id,
            "title": issue.title,
            "severity": issue.severity,
            "status": issue.status,
            "task_id": issue.task_id
        })).collect::<Vec<_>>(),
        "open_issues_more": digest.open_issues_more,
        "recent_notes": digest.recent_notes.iter().map(|note| json!({
            "id": note.id,
            "target": note.target,
            "target_id": note.target_id,
            "kind": note.kind,
            "body": note.body
        })).collect::<Vec<_>>(),
        "inventory": {
            "milestones": digest.inventory.milestones,
            "milestones_done": digest.inventory.milestones_done,
            "plans": digest.inventory.plans,
            "plans_done": digest.inventory.plans_done,
            "plans_on_hold": digest.inventory.plans_on_hold,
            "tasks": digest.inventory.tasks,
            "tasks_done": digest.inventory.tasks_done,
            "tasks_blocked": digest.inventory.tasks_blocked,
            "tasks_open": digest.inventory.tasks_open,
            "tasks_on_hold": digest.inventory.tasks_on_hold,
            "issues": digest.inventory.issues,
            "issues_open": digest.inventory.issues_open,
            "commits": digest.inventory.commits,
            "notes": digest.inventory.notes
        }
    })
}

fn next_value(view: &NextView) -> Value {
    json!({
        "goal": view.goal,
        "task": view.task.as_ref().map(task_line_value),
        "plan_title": view.plan_title,
        "message": view.message,
        "plan_hold_reason": view.plan_hold_reason,
        "plan_waiting_on": view.plan_waiting_on,
        "skipped": view.skipped.iter().map(|entry| json!({
            "task_id": entry.task_id,
            "waiting_on": entry.waiting_on
        })).collect::<Vec<_>>()
    })
}
