#![allow(
    clippy::needless_pass_by_value,
    clippy::needless_return,
    clippy::too_many_lines
)]

use std::ffi::OsString;
use std::path::PathBuf;

use clap::ArgMatches;
use ptrack_app::{
    ApplicationPort, GuideAction, HookAction, HookResult, InitRequest, Mutation, MutationResult,
};
use ptrack_core::{
    IssueStatus, MilestoneStatus, NoteTarget, PlanStatus, Severity, TaskStatus, Timestamp,
    board_for, check_hold_reason, context, hold_marker, next, search, show_issue, show_milestone,
    show_plan, show_task,
};

use crate::compat_json::{
    BoardJson, CommitJson, DigestJson, IssueJson, IssueShowJson, MilestoneJson, MilestoneShowJson,
    NextJson, NoteRow, PlanRow, PlanShowJson, ProjectJson, SearchJson, StatusJson, TaskRow,
    TaskShowJson, raw_or_null, timestamp,
};
use crate::error::CliError;
use crate::output;
use crate::parse::{Preflight, parse_flag_i64, parse_flag_u64, parse_u64, preflight};
use crate::{Io, RunOutcome};

const NO_ACTIVE_PLAN: &str = "no active plan; set one with 'ptrack plan use <id>' or pass --plan";

pub fn run<I, T>(
    args_os: I,
    application: &mut dyn ApplicationPort,
    mut io: Io<'_>,
) -> Result<RunOutcome, CliError>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let args = args_os
        .into_iter()
        .map(|value| value.into().into_string())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| CliError::message("command arguments must be valid UTF-8"))?;
    match preflight(args)? {
        Preflight::Help(path) => {
            crate::help::write(&path, io.stdout)?;
            return Ok(RunOutcome::ExitSuccess);
        }
        Preflight::UnknownHelpTopic(topic) => {
            crate::help::write(&[topic], io.stderr)?;
            return Ok(RunOutcome::ExitSuccess);
        }
        Preflight::Completion {
            shell,
            no_descriptions,
        } => {
            crate::completion::write(&shell, no_descriptions, io.stdout)?;
            return Ok(RunOutcome::ExitSuccess);
        }
        Preflight::GroupDefault(path) if path == ["--version"] => {
            output::line(
                io.stdout,
                format_args!("ptrack version {}", crate::version()),
            )?;
            return Ok(RunOutcome::ExitSuccess);
        }
        Preflight::GroupDefault(path) => {
            let snapshot = application.snapshot()?;
            let value = if path == ["summary"] {
                &snapshot.meta.summary
            } else {
                &snapshot.meta.goal
            };
            output::line(io.stdout, value)?;
            return Ok(RunOutcome::ExitSuccess);
        }
        Preflight::Run { argv, path } if path.is_empty() => {
            let _ = argv;
            return Ok(RunOutcome::LaunchTui);
        }
        Preflight::Run { argv, path } => {
            let matches = crate::tree::root()
                .try_get_matches_from(argv)
                .map_err(|error| normalize_clap_error(&error))?;
            dispatch(&path, &matches, application, &mut io)
        }
    }
}

fn dispatch(
    path: &[String],
    root: &ArgMatches,
    application: &mut dyn ApplicationPort,
    io: &mut Io<'_>,
) -> Result<RunOutcome, CliError> {
    let leaf = leaf_matches(root, path);
    match path
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["init"] => init(leaf, application, io),
        ["goal", "show"] => show_meta(false, application, io),
        ["goal", "set"] => set_meta(false, leaf, application),
        ["summary", "show"] => show_meta(true, application, io),
        ["summary", "set"] => set_meta(true, leaf, application),
        ["milestone", command] => milestone(command, leaf, application, io),
        ["plan", command] => plan(command, leaf, application, io),
        ["task", command] => task(command, leaf, application, io),
        ["issue", command] => issue(command, leaf, application, io),
        ["note", command] => note(command, leaf, application, io),
        ["commit", command] => commit(command, leaf, application, io),
        ["hook", command] => hook(command, application, io),
        ["context"] => context_command(leaf, application, io),
        ["guide"] => guide(leaf, application, io),
        ["next"] => next_command(leaf, application, io),
        ["search"] => search_command(leaf, application, io),
        ["board"] => board(leaf, application, io),
        ["gui"] => Ok(RunOutcome::LaunchGui {
            path: values(leaf, "path").first().cloned().unwrap_or_default(),
            plan_id: 0,
        }),
        ["status"] => status(leaf, application, io),
        ["projects"] => projects(leaf, application, io),
        ["backup"] => {
            let path = application.backup()?;
            output::line(io.stdout, path.display())?;
            Ok(RunOutcome::ExitSuccess)
        }
        ["capability", "call"] => capability_call(leaf, application, io),
        ["capability", "mcp"] => {
            let input = std::mem::replace(&mut io.stdin, Box::new(std::io::empty()));
            application.capability_mcp(input, io.stdout, &io.cancellation)?;
            Ok(RunOutcome::ExitSuccess)
        }
        ["version"] => {
            output::line(io.stdout, format_args!("ptrack {}", crate::version()))?;
            Ok(RunOutcome::ExitSuccess)
        }
        _ => Err(CliError::message("internal command dispatch mismatch")),
    }
}

fn init(
    matches: &ArgMatches,
    application: &mut dyn ApplicationPort,
    io: &mut Io<'_>,
) -> Result<RunOutcome, CliError> {
    let no_guide = matches.get_flag("no-guide");
    let result = application.initialize(InitRequest {
        root: option(matches, "root").map(PathBuf::from),
        goal: option(matches, "goal").cloned().unwrap_or_default(),
        force: matches.get_flag("force"),
        no_guide,
    })?;
    if result.already_initialized {
        output::line(
            io.stdout,
            format_args!(
                "project already initialized at {}",
                result.database.display()
            ),
        )?;
    } else {
        output::line(io.stdout, result.database.display())?;
    }
    if !no_guide {
        write_guide_result(io, &result.guide_files)?;
    }
    Ok(RunOutcome::ExitSuccess)
}

fn show_meta(
    summary: bool,
    application: &mut dyn ApplicationPort,
    io: &mut Io<'_>,
) -> Result<RunOutcome, CliError> {
    let snapshot = application.snapshot()?;
    output::line(
        io.stdout,
        if summary {
            &snapshot.meta.summary
        } else {
            &snapshot.meta.goal
        },
    )?;
    Ok(RunOutcome::ExitSuccess)
}

fn set_meta(
    summary: bool,
    matches: &ArgMatches,
    application: &mut dyn ApplicationPort,
) -> Result<RunOutcome, CliError> {
    let value = values(matches, "text").join(" ");
    let mutation = if summary {
        Mutation::SetSummary(value)
    } else {
        Mutation::SetGoal(value)
    };
    expect_none(application.mutate(mutation)?)?;
    Ok(RunOutcome::ExitSuccess)
}

fn milestone(
    command: &str,
    matches: &ArgMatches,
    application: &mut dyn ApplicationPort,
    io: &mut Io<'_>,
) -> Result<RunOutcome, CliError> {
    match command {
        "add" => {
            let due = option(matches, "due").map_or(Ok(Timestamp::Zero), |value| {
                parse_date(value).map_err(|error| {
                    CliError::message(format!(
                        "invalid --due {value:?} (want YYYY-MM-DD): {error}"
                    ))
                })
            })?;
            let result = application.mutate(Mutation::AddMilestone {
                title: values(matches, "title").join(" "),
                due,
            })?;
            let MutationResult::Milestone(value) = result else {
                return Err(internal_result());
            };
            output::line(
                io.stdout,
                format_args!("milestone #{} {}", value.id, value.title),
            )?;
        }
        "list" => {
            let snapshot = application.snapshot()?;
            if matches.get_flag("json") {
                let rows: Vec<MilestoneJson<'_>> =
                    snapshot.milestones.iter().map(Into::into).collect();
                output::json(io.stdout, &raw_or_null(rows))?;
            } else {
                for value in snapshot.milestones {
                    let due = value
                        .due
                        .stored_date()
                        .map_or_else(String::new, |date| format!(" (due {date})"));
                    output::line(
                        io.stdout,
                        format_args!("#{} [{}] {}{due}", value.id, value.status, value.title),
                    )?;
                }
            }
        }
        "show" => {
            let id = parse_u64(first(matches, "id")?)?;
            let view = show_milestone(&application.snapshot()?, id)?;
            if matches.get_flag("json") {
                output::json(io.stdout, &MilestoneShowJson::from(&view))?;
            } else {
                output::text(io.stdout, &view.markdown())?;
            }
        }
        "done" | "open" => {
            let id = parse_u64(first(matches, "id")?)?;
            let status = if command == "done" {
                MilestoneStatus::Done
            } else {
                MilestoneStatus::Open
            };
            expect_none(application.mutate(Mutation::SetMilestoneStatus { id, status })?)?;
        }
        "due" => {
            let args = values(matches, "values");
            let id = parse_u64(&args[0])?;
            let due = if args[1] == "-" {
                Timestamp::Zero
            } else {
                parse_date(&args[1]).map_err(|error| {
                    CliError::message(format!(
                        "invalid date {:?} (want YYYY-MM-DD): {error}",
                        args[1]
                    ))
                })?
            };
            expect_none(application.mutate(Mutation::SetMilestoneDue { id, due })?)?;
        }
        "rename" => {
            let args = values(matches, "values");
            expect_none(application.mutate(Mutation::SetMilestoneTitle {
                id: parse_u64(&args[0])?,
                title: args[1..].join(" "),
            })?)?;
        }
        _ => return Err(CliError::message("internal milestone dispatch mismatch")),
    }
    Ok(RunOutcome::ExitSuccess)
}

/// Joins and checks a hold reason at the input boundary, so the codec's
/// field-path message never reaches a person.
fn hold_reason(args: &[String]) -> Result<String, CliError> {
    let reason = args.join(" ");
    check_hold_reason(&reason).map_err(CliError::message)?;
    Ok(reason)
}

/// Store hold refusals already read as sentences ("task #3 is done and cannot
/// be put on hold"); drop the layer prefix rather than restate them.
fn hold_error(error: ptrack_app::AppError) -> CliError {
    let message = error.to_string();
    CliError::message(
        message
            .strip_prefix("invalid hold mutation: ")
            .unwrap_or(&message),
    )
}

fn plan(
    command: &str,
    matches: &ArgMatches,
    application: &mut dyn ApplicationPort,
    io: &mut Io<'_>,
) -> Result<RunOutcome, CliError> {
    match command {
        "add" => {
            let milestone_id = parse_flag_u64("milestone", option(matches, "milestone"))?;
            let result = application.mutate(Mutation::AddPlan {
                title: values(matches, "title").join(" "),
                milestone_id,
            })?;
            let MutationResult::Plan(value) = result else {
                return Err(internal_result());
            };
            output::line(
                io.stdout,
                format_args!("plan #{} {}", value.id, value.title),
            )?;
        }
        "list" => {
            let snapshot = application.snapshot()?;
            if matches.get_flag("json") {
                let rows: Vec<_> = snapshot
                    .plans
                    .iter()
                    .map(|plan| PlanRow {
                        id: plan.id,
                        title: &plan.title,
                        status: plan.status.as_str(),
                        active: plan.id == snapshot.meta.active_plan,
                        hold_reason: plan.hold_reason.as_deref(),
                    })
                    .collect();
                output::json(io.stdout, &rows)?;
            } else {
                for plan in snapshot.plans {
                    let mark = if plan.id == snapshot.meta.active_plan {
                        '*'
                    } else {
                        ' '
                    };
                    output::line(
                        io.stdout,
                        format_args!(
                            "#{} [{}] {mark} {}{}",
                            plan.id,
                            plan.status,
                            plan.title,
                            hold_marker(plan.hold_reason.as_deref())
                        ),
                    )?;
                }
            }
        }
        "show" => {
            let view = show_plan(&application.snapshot()?, parse_u64(first(matches, "id")?)?)?;
            if matches.get_flag("json") {
                output::json(io.stdout, &PlanShowJson::from(&view))?;
            } else {
                output::text(io.stdout, &view.markdown())?;
            }
        }
        "done" => {
            expect_none(application.mutate(Mutation::SetPlanStatus {
                id: parse_u64(first(matches, "id")?)?,
                status: PlanStatus::Done,
            })?)?;
        }
        "use" => {
            expect_none(
                application.mutate(Mutation::SetActivePlan(parse_u64(first(matches, "id")?)?))?,
            )?;
        }
        "hold" => {
            let args = values(matches, "values");
            let id = parse_u64(&args[0])?;
            let reason = hold_reason(&args[1..])?;
            expect_none(
                application
                    .mutate(Mutation::SetPlanHold {
                        id,
                        reason: Some(reason.clone()),
                    })
                    .map_err(hold_error)?,
            )?;
            output::line(io.stdout, format_args!("plan #{id} on hold: {reason}"))?;
        }
        "resume" => {
            let id = parse_u64(first(matches, "id")?)?;
            expect_none(application.mutate(Mutation::SetPlanHold { id, reason: None })?)?;
            output::line(io.stdout, format_args!("plan #{id} resumed"))?;
        }
        "rename" => {
            let args = values(matches, "values");
            expect_none(application.mutate(Mutation::SetPlanTitle {
                id: parse_u64(&args[0])?,
                title: args[1..].join(" "),
            })?)?;
        }
        _ => return Err(CliError::message("internal plan dispatch mismatch")),
    }
    Ok(RunOutcome::ExitSuccess)
}

fn task(
    command: &str,
    matches: &ArgMatches,
    application: &mut dyn ApplicationPort,
    io: &mut Io<'_>,
) -> Result<RunOutcome, CliError> {
    match command {
        "add" => {
            let mut plan_id = parse_flag_u64("plan", option(matches, "plan"))?;
            if plan_id == 0 {
                plan_id = application.snapshot()?.meta.active_plan;
                if plan_id == 0 {
                    return Err(CliError::message(NO_ACTIVE_PLAN));
                }
            }
            let result = application.mutate(Mutation::AddTask {
                plan_id,
                title: values(matches, "title").join(" "),
            })?;
            let MutationResult::Task(value) = result else {
                return Err(internal_result());
            };
            output::line(
                io.stdout,
                format_args!(
                    "task #{} {} (plan {})",
                    value.id, value.title, value.plan_id
                ),
            )?;
        }
        "list" => {
            let plan_id = parse_flag_u64("plan", option(matches, "plan"))?;
            let wanted = parse_statuses(option(matches, "status").map(String::as_str))?;
            let snapshot = application.snapshot()?;
            let tasks: Vec<_> = snapshot
                .tasks
                .iter()
                .filter(|task| plan_id == 0 || task.plan_id == plan_id)
                .filter(|task| wanted.as_ref().is_none_or(|set| set.contains(&task.status)))
                .collect();
            if matches.get_flag("json") {
                let rows: Vec<_> = tasks
                    .iter()
                    .map(|task| TaskRow {
                        id: task.id,
                        plan_id: task.plan_id,
                        title: &task.title,
                        status: task.status.as_str(),
                        hold_reason: task.hold_reason.as_deref(),
                    })
                    .collect();
                output::json(io.stdout, &rows)?;
            } else {
                for task in tasks {
                    output::line(
                        io.stdout,
                        format_args!(
                            "#{} [{}] {} (plan {}){}",
                            task.id,
                            task.status,
                            task.title,
                            task.plan_id,
                            hold_marker(task.hold_reason.as_deref())
                        ),
                    )?;
                }
            }
        }
        "show" => {
            let view = show_task(&application.snapshot()?, parse_u64(first(matches, "id")?)?)?;
            if matches.get_flag("json") {
                output::json(io.stdout, &TaskShowJson::from(&view))?;
            } else {
                output::text(io.stdout, &view.markdown())?;
            }
        }
        "start" | "done" | "block" => {
            let status = match command {
                "start" => TaskStatus::Doing,
                "done" => TaskStatus::Done,
                _ => TaskStatus::Blocked,
            };
            expect_none(application.mutate(Mutation::SetTaskStatus {
                id: parse_u64(first(matches, "id")?)?,
                status,
            })?)?;
        }
        "rename" => {
            let args = values(matches, "values");
            expect_none(application.mutate(Mutation::SetTaskTitle {
                id: parse_u64(&args[0])?,
                title: args[1..].join(" "),
            })?)?;
        }
        "hold" => {
            let args = values(matches, "values");
            let id = parse_u64(&args[0])?;
            let reason = hold_reason(&args[1..])?;
            expect_none(
                application
                    .mutate(Mutation::SetTaskHold {
                        id,
                        reason: Some(reason.clone()),
                    })
                    .map_err(hold_error)?,
            )?;
            output::line(io.stdout, format_args!("task #{id} on hold: {reason}"))?;
        }
        "resume" => {
            let id = parse_u64(first(matches, "id")?)?;
            expect_none(application.mutate(Mutation::SetTaskHold { id, reason: None })?)?;
            output::line(io.stdout, format_args!("task #{id} resumed"))?;
        }
        "move" => {
            let id = parse_u64(first(matches, "id")?)?;
            let plan_id = parse_flag_u64("plan", option(matches, "plan"))?;
            if plan_id == 0 {
                return Err(CliError::message("pass the target plan with --plan <id>"));
            }
            expect_none(application.mutate(Mutation::SetTaskPlan { id, plan_id })?)?;
            output::line(
                io.stdout,
                format_args!("task #{id} moved to plan {plan_id}"),
            )?;
        }
        "convert" => {
            let id = parse_u64(first(matches, "id")?)?;
            let result = application.mutate(Mutation::ConvertTaskToPlan(id))?;
            let MutationResult::Plan(value) = result else {
                return Err(internal_result());
            };
            output::line(
                io.stdout,
                format_args!("task #{id} converted to plan #{} {}", value.id, value.title),
            )?;
        }
        _ => return Err(CliError::message("internal task dispatch mismatch")),
    }
    Ok(RunOutcome::ExitSuccess)
}

fn issue(
    command: &str,
    matches: &ArgMatches,
    application: &mut dyn ApplicationPort,
    io: &mut Io<'_>,
) -> Result<RunOutcome, CliError> {
    match command {
        "add" => {
            let severity = parse_severity(option(matches, "severity").map(String::as_str))?;
            let task_id = parse_flag_u64("task", option(matches, "task"))?;
            let result = application.mutate(Mutation::AddIssue {
                title: values(matches, "title").join(" "),
                body: option(matches, "body").cloned().unwrap_or_default(),
                severity,
                task_id,
            })?;
            let MutationResult::Issue(value) = result else {
                return Err(internal_result());
            };
            output::line(
                io.stdout,
                format_args!("issue #{} [{}] {}", value.id, value.severity, value.title),
            )?;
        }
        "list" => {
            let status = match option(matches, "status").map(String::as_str) {
                None | Some("") => None,
                Some("open") => Some(IssueStatus::Open),
                Some("closed") => Some(IssueStatus::Closed),
                Some(value) => {
                    return Err(CliError::message(format!(
                        "invalid --status {value:?} (want open or closed)"
                    )));
                }
            };
            let snapshot = application.snapshot()?;
            let issues: Vec<_> = snapshot
                .issues
                .iter()
                .filter(|issue| status.is_none_or(|status| issue.status == status))
                .collect();
            if matches.get_flag("json") {
                let rows: Vec<_> = issues.iter().map(|issue| IssueJson::from(*issue)).collect();
                output::json(io.stdout, &raw_or_null(rows))?;
            } else {
                for issue in issues {
                    let link = if issue.task_id == 0 {
                        String::new()
                    } else {
                        format!(" (task {})", issue.task_id)
                    };
                    output::line(
                        io.stdout,
                        format_args!(
                            "#{} [{}] {} {}{link}",
                            issue.id, issue.severity, issue.status, issue.title
                        ),
                    )?;
                }
            }
        }
        "show" => {
            let view = show_issue(&application.snapshot()?, parse_u64(first(matches, "id")?)?)?;
            if matches.get_flag("json") {
                output::json(io.stdout, &IssueShowJson::from(&view))?;
            } else {
                output::text(io.stdout, &view.markdown())?;
            }
        }
        "close" | "open" => {
            let status = if command == "close" {
                IssueStatus::Closed
            } else {
                IssueStatus::Open
            };
            expect_none(application.mutate(Mutation::SetIssueStatus {
                id: parse_u64(first(matches, "id")?)?,
                status,
            })?)?;
        }
        "severity" => {
            let args = values(matches, "values");
            let severity = parse_severity(Some(&args[1]))?
                .ok_or_else(|| CliError::message("invalid empty severity"))?;
            expect_none(application.mutate(Mutation::SetIssueSeverity {
                id: parse_u64(&args[0])?,
                severity,
            })?)?;
        }
        "rename" => {
            let args = values(matches, "values");
            expect_none(application.mutate(Mutation::SetIssueTitle {
                id: parse_u64(&args[0])?,
                title: args[1..].join(" "),
            })?)?;
        }
        _ => return Err(CliError::message("internal issue dispatch mismatch")),
    }
    Ok(RunOutcome::ExitSuccess)
}

fn note(
    command: &str,
    matches: &ArgMatches,
    application: &mut dyn ApplicationPort,
    io: &mut Io<'_>,
) -> Result<RunOutcome, CliError> {
    match command {
        "add" => {
            let task_id = parse_flag_u64("task", option(matches, "task"))?;
            let plan_id = parse_flag_u64("plan", option(matches, "plan"))?;
            let (target, target_id) = if task_id != 0 {
                (NoteTarget::Task, task_id)
            } else if plan_id != 0 {
                (NoteTarget::Plan, plan_id)
            } else {
                (NoteTarget::Project, 0)
            };
            let result = application.mutate(Mutation::AddNote {
                target,
                target_id,
                body: values(matches, "text").join(" "),
            })?;
            let MutationResult::Note(value) = result else {
                return Err(internal_result());
            };
            output::line(io.stdout, format_args!("note #{} {}", value.id, value.body))?;
        }
        "list" => {
            let plan_id = parse_flag_u64("plan", option(matches, "plan"))?;
            let task_id = parse_flag_u64("task", option(matches, "task"))?;
            if plan_id != 0 && task_id != 0 {
                return Err(CliError::message(
                    "--plan and --task are mutually exclusive",
                ));
            }
            let limit = parse_flag_i64("limit", option(matches, "limit"), 20)?;
            let snapshot = application.snapshot()?;
            let mut notes: Vec<_> = snapshot
                .notes
                .iter()
                .rev()
                .filter(|note| {
                    (task_id == 0 || note.target == NoteTarget::Task && note.target_id == task_id)
                        && (plan_id == 0
                            || note.target == NoteTarget::Plan && note.target_id == plan_id)
                })
                .collect();
            if limit > 0 {
                notes.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
            }
            if matches.get_flag("json") {
                let rows: Vec<_> = notes
                    .iter()
                    .map(|note| NoteRow {
                        id: note.id,
                        target: note.target.as_str(),
                        target_id: note.target_id,
                        kind: note.kind.as_str(),
                        body: &note.body,
                    })
                    .collect();
                output::json(io.stdout, &rows)?;
            } else {
                for note in notes {
                    let kind = if note.kind.as_str().is_empty() {
                        String::new()
                    } else {
                        format!("{} · ", note.kind)
                    };
                    let target = if note.target_id == 0 {
                        note.target.to_string()
                    } else {
                        format!("{} #{}", note.target, note.target_id)
                    };
                    output::line(
                        io.stdout,
                        format_args!("#{} ({kind}{target}) {}", note.id, note.body),
                    )?;
                }
            }
        }
        _ => return Err(CliError::message("internal note dispatch mismatch")),
    }
    Ok(RunOutcome::ExitSuccess)
}

fn commit(
    command: &str,
    matches: &ArgMatches,
    application: &mut dyn ApplicationPort,
    io: &mut Io<'_>,
) -> Result<RunOutcome, CliError> {
    match command {
        "add" => {
            let args = values(matches, "values");
            let task_id = parse_flag_u64("task", option(matches, "task"))?;
            let requested_plan = parse_flag_u64("plan", option(matches, "plan"))?;
            let snapshot = application.snapshot()?;
            let (plan_id, task_id) = if task_id != 0 {
                let task = snapshot
                    .task(task_id)
                    .ok_or_else(|| CliError::message("not found"))?;
                (task.plan_id, task.id)
            } else if requested_plan != 0 {
                (requested_plan, 0)
            } else {
                (snapshot.meta.active_plan, 0)
            };
            let result = application.mutate(Mutation::AddCommit {
                sha: args[0].clone(),
                subject: args[1..].join(" "),
                plan_id,
                task_id,
            })?;
            let MutationResult::Commit(value) = result else {
                return Err(internal_result());
            };
            output::line(
                io.stdout,
                format_args!("commit {} recorded", short(&value.sha)),
            )?;
        }
        "record" => {
            let sha = option(matches, "sha").cloned().unwrap_or_default();
            if sha.is_empty() {
                return Err(CliError::message("--sha is required"));
            }
            let subject = option(matches, "subject").cloned().unwrap_or_default();
            let snapshot = application.snapshot()?;
            let task_id = task_reference(&subject)
                .filter(|id| snapshot.task(*id).is_some())
                .unwrap_or(0);
            let plan_id = snapshot
                .task(task_id)
                .map_or(snapshot.meta.active_plan, |task| task.plan_id);
            let _ = application.mutate(Mutation::AddCommit {
                sha,
                subject,
                plan_id,
                task_id,
            })?;
        }
        "list" => {
            let task_id = parse_flag_u64("task", option(matches, "task"))?;
            let plan_id = parse_flag_u64("plan", option(matches, "plan"))?;
            let snapshot = application.snapshot()?;
            let mut commits: Vec<_> = snapshot
                .commits
                .iter()
                .filter(|commit| {
                    if task_id != 0 {
                        commit.task_id == task_id
                    } else if plan_id != 0 {
                        commit.plan_id == plan_id
                    } else {
                        true
                    }
                })
                .collect();
            if task_id != 0 || plan_id != 0 {
                commits.reverse();
            }
            if matches.get_flag("json") {
                let rows: Vec<_> = commits
                    .iter()
                    .map(|commit| CommitJson::from(*commit))
                    .collect();
                output::json(io.stdout, &raw_or_null(rows))?;
            } else {
                for commit in commits {
                    let link = if commit.task_id != 0 {
                        format!(" (task {})", commit.task_id)
                    } else if commit.plan_id != 0 {
                        format!(" (plan {})", commit.plan_id)
                    } else {
                        String::new()
                    };
                    output::line(
                        io.stdout,
                        format_args!("{} {}{link}", short(&commit.sha), commit.subject),
                    )?;
                }
            }
        }
        "show" => {
            let argument = first(matches, "reference")?;
            let snapshot = application.snapshot()?;
            let reference = argument.parse::<u64>().ok().and_then(|id| {
                snapshot
                    .commits
                    .iter()
                    .find(|commit| commit.id == id)
                    .map(|commit| commit.sha.as_str())
            });
            let result =
                application.git_show(reference.unwrap_or(argument), matches.get_flag("stat"))?;
            io.stdout.write_all(&result.stdout)?;
            io.stderr.write_all(&result.stderr)?;
            if result.exit_code != Some(0) {
                let status = result.exit_code.map_or_else(
                    || "signal: process terminated".to_owned(),
                    |code| format!("exit status {code}"),
                );
                return Err(CliError::message(status));
            }
        }
        _ => return Err(CliError::message("internal commit dispatch mismatch")),
    }
    Ok(RunOutcome::ExitSuccess)
}

fn hook(
    command: &str,
    application: &mut dyn ApplicationPort,
    io: &mut Io<'_>,
) -> Result<RunOutcome, CliError> {
    let action = match command {
        "install" => HookAction::Install,
        "uninstall" => HookAction::Uninstall,
        "status" => HookAction::Status,
        _ => return Err(CliError::message("internal hook dispatch mismatch")),
    };
    match application.hook(action)? {
        HookResult::Installed { path, changed } if changed => output::line(
            io.stdout,
            format_args!("installed post-commit hook at {}", path.display()),
        )?,
        HookResult::Installed { .. } => {
            output::line(io.stdout, "post-commit hook already up to date")?;
        }
        HookResult::Removed => output::line(io.stdout, "removed ptrack post-commit hook")?,
        HookResult::Missing => output::line(io.stdout, "no post-commit hook")?,
        HookResult::Status { path, installed } if installed => {
            output::line(io.stdout, format_args!("installed: {}", path.display()))?;
        }
        HookResult::Status { .. } => {
            output::line(io.stdout, "not installed (run 'ptrack hook install')")?;
        }
    }
    Ok(RunOutcome::ExitSuccess)
}

fn context_command(
    matches: &ArgMatches,
    application: &mut dyn ApplicationPort,
    io: &mut Io<'_>,
) -> Result<RunOutcome, CliError> {
    let view = context(&application.snapshot()?);
    if matches.get_flag("json") {
        output::json(io.stdout, &DigestJson::from(&view))?;
    } else {
        output::text(io.stdout, &view.markdown())?;
    }
    Ok(RunOutcome::ExitSuccess)
}

fn guide(
    matches: &ArgMatches,
    application: &mut dyn ApplicationPort,
    io: &mut Io<'_>,
) -> Result<RunOutcome, CliError> {
    let print = matches.get_flag("print");
    let (rendered, files) = application.guide(if print {
        GuideAction::Print
    } else {
        GuideAction::Install
    })?;
    if print {
        output::text(io.stdout, &rendered)?;
    } else {
        write_guide_result(io, &files)?;
    }
    Ok(RunOutcome::ExitSuccess)
}

fn write_guide_result(io: &mut Io<'_>, files: &[PathBuf]) -> Result<(), CliError> {
    if files.is_empty() {
        output::line(io.stdout, "agent guide already up to date")?;
    } else {
        for path in files {
            output::line(
                io.stdout,
                format_args!("wrote agent guide to {}", path.display()),
            )?;
        }
    }
    Ok(())
}

fn next_command(
    matches: &ArgMatches,
    application: &mut dyn ApplicationPort,
    io: &mut Io<'_>,
) -> Result<RunOutcome, CliError> {
    let view = next(&application.snapshot()?)?;
    if matches.get_flag("json") {
        output::json(io.stdout, &NextJson::from(&view))?;
    } else {
        output::text(io.stdout, &view.markdown())?;
    }
    Ok(RunOutcome::ExitSuccess)
}

fn search_command(
    matches: &ArgMatches,
    application: &mut dyn ApplicationPort,
    io: &mut Io<'_>,
) -> Result<RunOutcome, CliError> {
    let view = search(&application.snapshot()?, &values(matches, "term").join(" "));
    if matches.get_flag("json") {
        output::json(io.stdout, &SearchJson::from(&view))?;
    } else {
        output::text(io.stdout, &view.markdown())?;
    }
    Ok(RunOutcome::ExitSuccess)
}

fn board(
    matches: &ArgMatches,
    application: &mut dyn ApplicationPort,
    io: &mut Io<'_>,
) -> Result<RunOutcome, CliError> {
    let mut plan_id = parse_flag_u64("plan", option(matches, "plan"))?;
    if matches.get_flag("gui") {
        if matches.get_flag("json") {
            return Err(CliError::message(
                "--gui and --json cannot be used together",
            ));
        }
        return Ok(RunOutcome::LaunchGui {
            path: String::new(),
            plan_id,
        });
    }
    let snapshot = application.snapshot()?;
    if plan_id == 0 {
        plan_id = snapshot.meta.active_plan;
        if plan_id == 0 {
            return Err(CliError::message(NO_ACTIVE_PLAN));
        }
    }
    let view = board_for(&snapshot, plan_id)?;
    if matches.get_flag("json") {
        output::json(io.stdout, &BoardJson::from(&view))?;
    } else {
        output::text(io.stdout, &view.markdown())?;
    }
    Ok(RunOutcome::ExitSuccess)
}

fn status(
    matches: &ArgMatches,
    application: &mut dyn ApplicationPort,
    io: &mut Io<'_>,
) -> Result<RunOutcome, CliError> {
    let snapshot = application.snapshot()?;
    let active_title = snapshot
        .plan(snapshot.meta.active_plan)
        .map_or("", |plan| plan.title.as_str());
    let (mut todo, mut doing, mut done, mut blocked) = (0, 0, 0, 0);
    for task in &snapshot.tasks {
        match task.status {
            TaskStatus::Todo => todo += 1,
            TaskStatus::Doing => doing += 1,
            TaskStatus::Done => done += 1,
            TaskStatus::Blocked => blocked += 1,
        }
    }
    // A hold is orthogonal to status: a held task still counts under its own
    // status above and again here.
    let on_hold = snapshot.counts().tasks_on_hold;
    if matches.get_flag("json") {
        output::json(
            io.stdout,
            &StatusJson {
                goal: &snapshot.meta.goal,
                active_plan: snapshot.meta.active_plan,
                active_plan_title: active_title,
                plans: snapshot.plans.len(),
                todo,
                doing,
                done,
                blocked,
                on_hold,
            },
        )?;
    } else {
        let goal = snapshot
            .meta
            .goal
            .split_once('\n')
            .map_or(snapshot.meta.goal.as_str(), |(first, _)| first)
            .trim();
        output::line(
            io.stdout,
            format_args!(
                "goal: {}",
                if goal.is_empty() {
                    "(no goal set)"
                } else {
                    goal
                }
            ),
        )?;
        output::line(
            io.stdout,
            format_args!(
                "active plan: {}",
                if active_title.is_empty() {
                    "(no active plan)"
                } else {
                    active_title
                }
            ),
        )?;
        let held = if on_hold == 0 {
            String::new()
        } else {
            format!(" ({on_hold} on hold)")
        };
        output::line(
            io.stdout,
            format_args!("tasks: {todo} todo, {doing} doing, {done} done, {blocked} blocked{held}"),
        )?;
        output::line(io.stdout, format_args!("plans: {}", snapshot.plans.len()))?;
    }
    Ok(RunOutcome::ExitSuccess)
}

fn projects(
    matches: &ArgMatches,
    application: &mut dyn ApplicationPort,
    io: &mut Io<'_>,
) -> Result<RunOutcome, CliError> {
    let projects = application.projects()?;
    if matches.get_flag("json") {
        let rows: Vec<_> = projects
            .iter()
            .map(Into::into)
            .collect::<Vec<ProjectJson<'_>>>();
        output::json(io.stdout, &raw_or_null(rows))?;
    } else {
        for project in projects {
            let seen = timestamp(project.last_seen);
            let seen = seen.get(..19).unwrap_or(&seen).replace('T', " ");
            output::line(
                io.stdout,
                format_args!("{}\t{}\t{seen}", project.name, project.path),
            )?;
        }
    }
    Ok(RunOutcome::ExitSuccess)
}

fn capability_call(
    matches: &ArgMatches,
    application: &mut dyn ApplicationPort,
    io: &mut Io<'_>,
) -> Result<RunOutcome, CliError> {
    let arguments = option(matches, "arguments").map_or("{}", String::as_str);
    let valid =
        serde_json::from_str::<serde_json::Value>(arguments).is_ok_and(|value| value.is_object());
    if !valid {
        return Err(CliError::message("--arguments must be one JSON object"));
    }
    let result = application.capability_call(first(matches, "tool")?, arguments)?;
    io.stdout.write_all(&result)?;
    io.stdout.write_all(b"\n")?;
    Ok(RunOutcome::ExitSuccess)
}

fn leaf_matches<'a>(root: &'a ArgMatches, path: &[String]) -> &'a ArgMatches {
    let mut matches = root;
    for component in path {
        matches = matches
            .subcommand_matches(component)
            .expect("preflight path exists in clap tree");
    }
    matches
}

fn values(matches: &ArgMatches, name: &str) -> Vec<String> {
    matches
        .get_many::<String>(name)
        .map(|values| values.cloned().collect())
        .unwrap_or_default()
}

fn first<'a>(matches: &'a ArgMatches, name: &str) -> Result<&'a str, CliError> {
    matches
        .get_one::<String>(name)
        .map(String::as_str)
        .ok_or_else(|| CliError::message("internal missing command argument"))
}

fn option<'a>(matches: &'a ArgMatches, name: &str) -> Option<&'a String> {
    matches.get_one::<String>(name)
}

fn expect_none(result: MutationResult) -> Result<(), CliError> {
    if result == MutationResult::None {
        Ok(())
    } else {
        Err(internal_result())
    }
}

fn internal_result() -> CliError {
    CliError::message("internal application result mismatch")
}

fn normalize_clap_error(error: &clap::Error) -> CliError {
    let rendered = error.to_string();
    let first = rendered
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("invalid command line")
        .trim_start_matches("error: ");
    CliError::message(first)
}

fn parse_statuses(value: Option<&str>) -> Result<Option<Vec<TaskStatus>>, CliError> {
    let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
        return Ok(None);
    };
    let mut statuses = Vec::new();
    for part in value.split(',') {
        let status = match part.trim() {
            "todo" => TaskStatus::Todo,
            "doing" => TaskStatus::Doing,
            "done" => TaskStatus::Done,
            "blocked" => TaskStatus::Blocked,
            _ => {
                return Err(CliError::message(format!(
                    "invalid status {part:?} (want todo,doing,done,blocked)"
                )));
            }
        };
        if !statuses.contains(&status) {
            statuses.push(status);
        }
    }
    Ok(Some(statuses))
}

fn parse_severity(value: Option<&str>) -> Result<Option<Severity>, CliError> {
    match value.unwrap_or_default() {
        "" => Ok(None),
        "low" => Ok(Some(Severity::Low)),
        "medium" => Ok(Some(Severity::Medium)),
        "high" => Ok(Some(Severity::High)),
        "critical" => Ok(Some(Severity::Critical)),
        value => Err(CliError::message(format!(
            "invalid severity {value:?} (want low, medium, high, critical)"
        ))),
    }
}

fn short(sha: &str) -> &str {
    sha.get(..8).unwrap_or(sha)
}

fn task_reference(subject: &str) -> Option<u64> {
    let bytes = subject.as_bytes();
    for index in 0..bytes.len() {
        if bytes[index] != b'#' {
            continue;
        }
        let mut end = index + 1;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        if end > index + 1 {
            return subject[index + 1..end].parse().ok();
        }
    }
    None
}

fn parse_date(value: &str) -> Result<Timestamp, String> {
    let bytes = value.as_bytes();
    let parse_error = |remaining: &str, layout: &str| {
        format!(
            "parsing time {value:?} as \"2006-01-02\": cannot parse {remaining:?} as {layout:?}"
        )
    };
    if bytes.len() < 4 || !bytes[..4].iter().all(u8::is_ascii_digit) {
        return Err(parse_error(value, "2006"));
    }
    if bytes.get(4) != Some(&b'-') {
        return Err(parse_error(value.get(4..).unwrap_or_default(), "-"));
    }
    if bytes.len() < 7 || !bytes[5..7].iter().all(u8::is_ascii_digit) {
        return Err(parse_error(value.get(5..).unwrap_or_default(), "01"));
    }
    if bytes.get(7) != Some(&b'-') {
        return Err(parse_error(value.get(7..).unwrap_or_default(), "-"));
    }
    if bytes.len() < 10 || !bytes[8..10].iter().all(u8::is_ascii_digit) {
        return Err(parse_error(value.get(8..).unwrap_or_default(), "02"));
    }
    if bytes.len() > 10 {
        return Err(format!(
            "parsing time {value:?}: extra text: {:?}",
            &value[10..]
        ));
    }
    let year = value[0..4]
        .parse::<i64>()
        .map_err(|_| parse_error(value, "2006"))?;
    let month = value[5..7]
        .parse::<u8>()
        .map_err(|_| parse_error(&value[5..], "01"))?;
    let day = value[8..10]
        .parse::<u8>()
        .map_err(|_| parse_error(&value[8..], "02"))?;
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let maximum = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return Err(format!("parsing time {value:?}: month out of range")),
    };
    if day == 0 || day > maximum {
        return Err(format!("parsing time {value:?}: day out of range"));
    }
    let year_adjusted = year - i64::from(month <= 2);
    let era = year_adjusted.div_euclid(400);
    let year_of_era = year_adjusted - era * 400;
    let shifted_month = i64::from(month) + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    Ok(Timestamp::Fixed {
        seconds: days
            .checked_mul(86_400)
            .ok_or_else(|| format!("parsing time {value:?}: day out of range"))?,
        nanoseconds: 0,
        offset_seconds: 0,
    })
}
