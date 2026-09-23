use std::collections::BTreeSet;

use serde_json::{Value, json};

use super::allowed_desktop_commands;
use super::command::{
    AgentCommand, DesktopCommand, LinkedAgentCommand, RecentCommand, TaskCommand, TerminalCommand,
    WorkspaceCommand,
};

/// A 43-byte recent-project identifier, the only shape the registry accepts.
const RECENT_ID: &str = "abcdefghijklmnopqrstuvwxyz0123456789-_ABCDE";

/// One well-formed argument list for every allowlisted method.
#[allow(clippy::too_many_lines)] // One row per allowlisted method.
fn well_formed() -> Vec<(&'static str, Vec<Value>)> {
    let pointer = json!({ "version": 1, "planId": 0, "taskId": 0 });
    vec![
        (
            "AcknowledgeAgentHandoffV2",
            vec![json!(7), json!("id"), json!("run")],
        ),
        (
            "AddIssueV1",
            vec![json!(7), json!("Issue"), json!("Body"), json!("low")],
        ),
        ("AddPlanV1", vec![json!(7), json!("Plan")]),
        ("AddTaskNoteV2", vec![json!(7), json!(1), json!("Note")]),
        ("AddTaskV2", vec![json!(7), json!(1), json!("Task")]),
        ("ApplyUpdate", vec![json!("1.0.0")]),
        ("ApproveAgentWorkflowV2", vec![json!(7), json!("id")]),
        ("CancelUpdateOperation", vec![]),
        ("CancelWorkspaceChange", vec![json!("token")]),
        ("CheckForUpdates", vec![]),
        ("ClaimTerminalStream", vec![json!("session"), json!(0)]),
        ("CloseProject", vec![json!("")]),
        (
            "CloseTerminalV2",
            vec![json!(7), json!("session"), json!(false)],
        ),
        ("CompletePlanV1", vec![json!(7), json!(1)]),
        ("CopyPlanV1", vec![json!(7), json!(1), json!(""), json!("")]),
        ("CreateFirstPlanV1", vec![json!(7), json!("Plan")]),
        ("CreateFirstTaskV1", vec![json!(7), json!(1), json!("Task")]),
        (
            "CreateTerminalV2",
            vec![json!(7), json!("shell"), json!(""), json!(24), json!(80)],
        ),
        ("DeletePlanV1", vec![json!(7), json!(1), json!(false)]),
        ("DismissAgentWorkflowV2", vec![json!(7), json!("id")]),
        ("DownloadUpdate", vec![json!("1.0.0")]),
        (
            "ForgetRecentProjectV1",
            vec![json!(RECENT_ID), json!(RECENT_ID)],
        ),
        ("GetActivityHeatmapV2", vec![json!(16)]),
        ("GetDiagnosticsReport", vec![]),
        ("GetGlobalOverviewV1", vec![]),
        ("GetInitializationStatusV1", vec![json!("operation")]),
        ("GetIssueDetailV1", vec![json!(7), json!(1)]),
        ("GetIssuesV1", vec![json!(7), json!("all"), json!(0)]),
        ("GetLayoutState", vec![]),
        ("GetPendingInitializationV1", vec![]),
        ("GetPreferences", vec![]),
        ("GetProjectTimelineV1", vec![]),
        ("GetRecentProjectsV1", vec![]),
        ("GetScratchpadV1", vec![json!(7)]),
        ("GetStackProfileV1", vec![json!(false)]),
        ("GetTaskDetailV2", vec![json!(7), json!(1)]),
        ("GetTerminalProfiles", vec![]),
        ("GetTerminalProfilesV2", vec![json!(7)]),
        ("GetTerminalWindowTab", vec![json!("terminal-1")]),
        ("GetUpdateState", vec![]),
        ("GetWorkspaceSnapshot", vec![json!(7), Value::Null]),
        ("GetWorkspaceState", vec![]),
        ("HoldPlanV1", vec![json!(7), json!(1), json!("Waiting")]),
        (
            "InitializeProjectV1",
            vec![json!({ "operationId": "operation", "root": "/project", "goal": "Ship" })],
        ),
        ("InstallShellCommand", vec![]),
        (
            "LaunchLinkedAgentV2",
            vec![
                json!(7),
                json!("agent"),
                json!(""),
                json!(24),
                json!(80),
                pointer,
            ],
        ),
        ("ListProjectsV1", vec![json!(7)]),
        (
            "MoveIssueTaskV1",
            vec![json!(7), json!(1), json!(2), json!(3), json!(4)],
        ),
        (
            "MovePlanV1",
            vec![json!(7), json!(1), json!("/other"), json!("")],
        ),
        (
            "MoveTaskV3",
            vec![json!(7), json!(1), json!("done"), json!("")],
        ),
        (
            "MutateTerminalAssociationV2",
            vec![json!(7), json!("session"), json!(1), json!(true)],
        ),
        ("OpenHelpDestination", vec![json!("help-center")]),
        ("OpenProject", vec![json!("/project"), json!("")]),
        (
            "OpenRecentProjectV1",
            vec![
                json!(RECENT_ID),
                json!(RECENT_ID),
                json!("/project"),
                json!(""),
                json!(""),
            ],
        ),
        ("OpenTerminalWindow", vec![json!(["session"]), json!({})]),
        ("PickProjectDirectory", vec![]),
        (
            "PrepareAgentWorkflowV2",
            vec![
                json!(7),
                json!("run"),
                json!(1),
                json!("commit"),
                json!("main"),
            ],
        ),
        ("PreviewAgentHandoffV2", vec![json!(7), json!("run")]),
        (
            "PreviewProjectGuideV1",
            vec![json!({ "operationId": "operation", "root": "/project" })],
        ),
        (
            "PreviewTerminalWritebackV2",
            vec![
                json!(7),
                json!("session"),
                json!(1),
                json!("decision"),
                json!("Text"),
            ],
        ),
        ("RefreshGlobalOverviewV1", vec![]),
        ("RenamePlanV1", vec![json!(7), json!(1), json!("Plan")]),
        ("RenameTaskV2", vec![json!(7), json!(1), json!("Task")]),
        ("ReopenPlanV1", vec![json!(7), json!(1)]),
        ("ResetApplicationState", vec![]),
        ("ResetPreferences", vec![]),
        ("ResetWindowLayout", vec![]),
        (
            "ResizeTerminalV2",
            vec![json!(7), json!("session"), json!(24), json!(80)],
        ),
        (
            "ResolveRecentProjectV1",
            vec![json!(RECENT_ID), json!(RECENT_ID), json!("/project")],
        ),
        ("ResumePlanV1", vec![json!(7), json!(1)]),
        (
            "RollbackLinkedAgentLaunchV2",
            vec![json!(7), json!("session")],
        ),
        (
            "ScheduleIssueV1",
            vec![json!(7), json!(1), json!(2), json!("")],
        ),
        ("SearchV2", vec![json!("query")]),
        (
            "SendAgentHandoffV2",
            vec![
                json!(7),
                json!("source"),
                json!("target"),
                json!(1),
                json!(2),
            ],
        ),
        (
            "SetAgentTaskOwnershipV2",
            vec![json!(7), json!("run"), json!(1), json!(true)],
        ),
        (
            "SetAgentWorktreeV2",
            vec![
                json!(7),
                json!("run"),
                json!(1),
                json!("/worktree"),
                json!(true),
            ],
        ),
        ("SetAutomaticUpdateChecks", vec![json!(true)]),
        (
            "SetIssueTaskV1",
            vec![json!(7), json!(1), json!(0), json!(2)],
        ),
        ("SetLayoutState", vec![json!({})]),
        ("SetPreferences", vec![json!({})]),
        ("SetScratchpadV1", vec![json!(7), json!(0), json!({})]),
        (
            "SetTerminalWindowTab",
            vec![json!("terminal-1"), json!(["session"]), json!({})],
        ),
        (
            "StartFirstTaskV1",
            vec![json!(7), json!(1), json!("2026-01-01T00:00:00Z")],
        ),
        (
            "UpdateIssueV1",
            vec![
                json!(7),
                json!(1),
                json!("Issue"),
                json!("Body"),
                json!("low"),
                json!("open"),
                json!("0001-01-01T00:00:00Z"),
            ],
        ),
        ("ValidateProjectTargetV1", vec![json!("/project")]),
        (
            "ValidateTerminalCWDsV2",
            vec![json!(7), json!(["/project"])],
        ),
        (
            "WriteTerminalMemoryV2",
            vec![
                json!(7),
                json!("session"),
                json!(1),
                json!("request"),
                json!("decision"),
                json!("Text"),
            ],
        ),
    ]
}

fn desktop_error(method: &str, arguments: &[Value]) -> String {
    DesktopCommand::parse(method, arguments)
        .expect_err("malformed request must not parse")
        .to_string()
}

fn workspace_error(method: &str, arguments: &[Value]) -> String {
    WorkspaceCommand::parse(method, arguments)
        .expect_err("malformed request must not parse")
        .to_string()
}

#[test]
fn every_allowlisted_method_parses_from_well_formed_arguments() {
    let table = well_formed();
    let covered = table
        .iter()
        .map(|(method, _)| *method)
        .collect::<BTreeSet<_>>();
    let allowed = allowed_desktop_commands()
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    assert_eq!(
        covered, allowed,
        "the table must cover the allowlist exactly"
    );
    for (method, arguments) in &table {
        let command = DesktopCommand::parse(method, arguments)
            .unwrap_or_else(|error| panic!("{method}: {error}"));
        if matches!(command, DesktopCommand::Workspace) {
            WorkspaceCommand::parse(method, arguments)
                .unwrap_or_else(|error| panic!("{method}: {error}"));
        } else {
            assert_eq!(
                workspace_error(method, arguments),
                format!("{method} is unavailable"),
                "{method} must be answered by exactly one scope"
            );
        }
    }
}

#[test]
fn unknown_workspace_methods_are_unavailable_without_substring_routing() {
    for method in ["GetBoardV2", "FrobnicateAgentV2", "GetCapabilitiesV2"] {
        assert_eq!(
            workspace_error(method, &[json!(7), json!(1)]),
            format!("{method} is unavailable")
        );
    }
}

#[test]
fn exact_arity_commands_report_the_historical_count_error() {
    for (method, expected) in [
        ("GetPreferences", 0),
        ("SetPreferences", 1),
        ("GetLayoutState", 0),
        ("SetLayoutState", 1),
        ("ResetApplicationState", 0),
        ("OpenTerminalWindow", 2),
        ("GetTerminalWindowTab", 1),
        ("SetTerminalWindowTab", 3),
        ("GetPendingInitializationV1", 0),
        ("GetGlobalOverviewV1", 0),
        ("ResolveRecentProjectV1", 3),
        ("ForgetRecentProjectV1", 2),
        ("OpenRecentProjectV1", 5),
    ] {
        let arguments = vec![json!(0); expected + 1];
        assert_eq!(
            desktop_error(method, &arguments),
            format!("{method} requires exactly {expected} arguments")
        );
    }
    for (method, expected) in [
        ("AddPlanV1", 2),
        ("RenamePlanV1", 3),
        ("CopyPlanV1", 4),
        ("CreateFirstTaskV1", 3),
        ("GetIssuesV1", 3),
        ("UpdateIssueV1", 7),
        ("MoveIssueTaskV1", 5),
        ("ListProjectsV1", 1),
        ("GetProjectTimelineV1", 0),
        ("GetStackProfileV1", 1),
        ("ClaimTerminalStream", 2),
    ] {
        let arguments = vec![json!(0); expected + 1];
        assert_eq!(
            workspace_error(method, &arguments),
            format!("{method} requires exactly {expected} arguments")
        );
    }
}

#[test]
fn optional_trailing_arguments_keep_their_own_count_errors() {
    assert_eq!(
        workspace_error("DeletePlanV1", &[json!(7), json!(1)]),
        "plan delete expects generation, plan ID, confirmation, and optional preview revision"
    );
    assert_eq!(
        workspace_error("GetIssueDetailV1", &[json!(7)]),
        "issue detail expects generation, issue ID, and optional target search"
    );
}

#[test]
fn mistyped_arguments_report_the_historical_argument_error() {
    for (method, arguments, index) in [
        ("AddTaskV2", vec![json!("7"), json!(1), json!("Task")], 0),
        (
            "CloseTerminalV2",
            vec![json!(7), json!("session"), json!(1)],
            2,
        ),
        ("GetActivityHeatmapV2", vec![json!("16")], 0),
        (
            "CreateTerminalV2",
            vec![
                json!(7),
                json!("shell"),
                json!(""),
                json!(70_000),
                json!(80),
            ],
            3,
        ),
        (
            "SetScratchpadV1",
            vec![json!(7), json!(0), json!("text")],
            2,
        ),
        ("GetWorkspaceSnapshot", vec![json!(7)], 1),
    ] {
        assert_eq!(
            workspace_error(method, &arguments),
            format!("desktop command argument {index} is invalid"),
            "{method}"
        );
    }
    assert_eq!(
        workspace_error("ValidateTerminalCWDsV2", &[json!(7), json!(["/a", 3])]),
        "desktop command argument 1[1] is invalid"
    );
    assert_eq!(
        desktop_error("SetAutomaticUpdateChecks", &[json!("yes")]),
        "desktop command argument 0 is invalid"
    );
    assert_eq!(
        desktop_error("InitializeProjectV1", &[json!({ "root": "/project" })]),
        "desktop command argument 0 is invalid"
    );
    assert_eq!(
        desktop_error(
            "ResolveRecentProjectV1",
            &[json!("short"), json!(RECENT_ID), json!("/project")]
        ),
        "desktop command argument 0 is invalid"
    );
    assert_eq!(
        desktop_error("OpenTerminalWindow", &[json!(["session"]), json!([])]),
        "OpenTerminalWindow requires an object tab shape"
    );
}

#[test]
fn arguments_are_checked_in_the_order_the_handlers_read_them() {
    // The title (argument 2) was always read before the plan (argument 1).
    assert_eq!(
        workspace_error("AddTaskV2", &[json!(7), json!("plan"), json!(3)]),
        "desktop command argument 2 is invalid"
    );
    // The workflow kind (argument 3) was always read before the run.
    assert_eq!(
        workspace_error(
            "PrepareAgentWorkflowV2",
            &[json!(7), json!(1), json!(1), json!(4), json!("main")]
        ),
        "desktop command argument 3 is invalid"
    );
}

#[test]
fn a_malformed_recent_open_target_is_kept_for_the_handler() {
    let arguments = [
        json!("short"),
        json!(RECENT_ID),
        json!("/project"),
        json!(""),
        json!(RECENT_ID),
    ];
    let command = DesktopCommand::parse("OpenRecentProjectV1", &arguments).unwrap();
    let DesktopCommand::Recent(RecentCommand::Open {
        workspace_token,
        target,
    }) = command
    else {
        panic!("OpenRecentProjectV1 must parse as a recent open");
    };
    assert_eq!(workspace_token, RECENT_ID);
    assert_eq!(
        target.unwrap_err().to_string(),
        "desktop command argument 0 is invalid"
    );
}

#[test]
fn conditional_arguments_are_required_only_by_the_form_that_reads_them() {
    let detach_arguments = [json!(7), json!("session"), json!(1), json!(true)];
    let detach = WorkspaceCommand::parse("MutateTerminalAssociationV2", &detach_arguments).unwrap();
    assert!(matches!(
        detach,
        WorkspaceCommand::Terminal(TerminalCommand::MutateAssociation { detach: true, .. })
    ));
    assert_eq!(
        workspace_error(
            "MutateTerminalAssociationV2",
            &[json!(7), json!("session"), json!(1), json!(false)]
        ),
        "desktop command argument 4 is invalid"
    );
    let write_arguments = [
        json!(7),
        json!("session"),
        json!(1),
        json!("request"),
        json!("summary"),
        json!("Text"),
    ];
    let write = WorkspaceCommand::parse("WriteTerminalMemoryV2", &write_arguments).unwrap();
    assert!(matches!(
        write,
        WorkspaceCommand::Terminal(TerminalCommand::WriteMemory {
            confirm_summary: None,
            ..
        })
    ));
}

#[test]
fn twin_profile_commands_share_one_typed_command() {
    assert!(matches!(
        WorkspaceCommand::parse("GetTerminalProfiles", &[]).unwrap(),
        WorkspaceCommand::Terminal(TerminalCommand::Profiles { generation: None })
    ));
    assert!(matches!(
        WorkspaceCommand::parse("GetTerminalProfilesV2", &[json!(7)]).unwrap(),
        WorkspaceCommand::Terminal(TerminalCommand::Profiles {
            generation: Some(7)
        })
    ));
}

#[test]
fn typed_fields_carry_the_positional_values() {
    assert!(matches!(
        WorkspaceCommand::parse(
            "MoveTaskV3",
            &[json!(7), json!(3), json!("doing"), json!("token")]
        )
        .unwrap(),
        WorkspaceCommand::Task(TaskCommand::Move {
            generation: 7,
            task_id: 3,
            status: "doing",
            confirmation_token: "token",
        })
    ));
    assert!(matches!(
        WorkspaceCommand::parse(
            "LaunchLinkedAgentV2",
            &[
                json!(7),
                json!("agent"),
                json!("sub"),
                json!(30),
                json!(100),
                json!({ "version": 1, "planId": 2, "taskId": 5 }),
            ]
        )
        .unwrap(),
        WorkspaceCommand::Agent(AgentCommand::Linked(LinkedAgentCommand::Launch {
            generation: 7,
            profile_id: "agent",
            cwd: "sub",
            rows: 30,
            columns: 100,
            ..
        }))
    ));
}
