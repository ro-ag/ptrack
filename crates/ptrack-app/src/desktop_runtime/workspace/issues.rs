//! Issue commands: the filtered list, the detail drawer, and the mutations
//! that file, edit, link, move, and schedule an issue.

use ptrack_core::{IssueStatus, Severity, Timestamp};
use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::super::SNAPSHOT_ISSUE_LIMIT;
use super::super::command::IssueCommand;
use super::super::ports::DesktopWorkspace;
use super::super::projection::{
    issue_detail_summary, issue_detail_value, issue_record_value, task_card,
};
use super::super::support::{bound, lock, message, trimmed_nonempty, unavailable};
use super::BoundDesktopWorkspace;
use crate::{AppError, AppResult, Mutation, MutationResult};

impl BoundDesktopWorkspace {
    pub(super) fn issue_command(&self, command: IssueCommand<'_>) -> AppResult<Value> {
        match command {
            IssueCommand::List {
                generation,
                filter,
                offset,
            } => {
                self.require_exact_generation(generation)?;
                self.issues_v1(filter, offset)
            }
            IssueCommand::Detail {
                generation,
                issue_id,
                query,
            } => {
                self.require_exact_generation(generation)?;
                issue_detail_value(self.generation, &self.snapshot()?, issue_id, query)
            }
            IssueCommand::Add {
                generation,
                title,
                body,
                severity,
            } => {
                self.require_exact_generation(generation)?;
                let title = trimmed_nonempty(title, "issue title cannot be empty")?;
                let severity = parse_issue_severity(severity)?;
                let result = lock(&self.application).mutate(Mutation::AddIssue {
                    title,
                    body: body.to_owned(),
                    severity: Some(severity),
                    task_id: 0,
                })?;
                issue_mutation_value(self.generation, result, "issue mutation")
            }
            IssueCommand::Update {
                generation,
                issue_id,
                title,
                body,
                severity,
                status,
                expected_updated_at,
            } => {
                self.require_exact_generation(generation)?;
                let expected_updated_at = parse_issue_timestamp(expected_updated_at)?;
                let title = trimmed_nonempty(title, "issue title cannot be empty")?;
                let severity = parse_issue_severity(severity)?;
                let status = parse_issue_status(status)?;
                let result = lock(&self.application).mutate(Mutation::UpdateIssue {
                    id: issue_id,
                    expected_updated_at,
                    title,
                    body: body.to_owned(),
                    severity,
                    status,
                })?;
                issue_mutation_value(self.generation, result, "issue mutation")
            }
            IssueCommand::SetTask {
                generation,
                issue_id,
                expected_task_id,
                task_id,
            } => {
                self.require_exact_generation(generation)?;
                let result = lock(&self.application).mutate(Mutation::SetIssueTask {
                    id: issue_id,
                    expected_task_id,
                    task_id,
                })?;
                issue_mutation_value(self.generation, result, "issue mutation")
            }
            IssueCommand::MoveTask {
                generation,
                issue_id,
                expected_task_id,
                expected_plan_id,
                plan_id,
            } => {
                self.require_exact_generation(generation)?;
                self.move_issue_task_v1(issue_id, expected_task_id, expected_plan_id, plan_id)
            }
            IssueCommand::Schedule {
                generation,
                issue_id,
                plan_id,
                title,
            } => {
                self.require_exact_generation(generation)?;
                self.schedule_issue_v1(issue_id, plan_id, title)
            }
        }
    }

    fn issues_v1(&self, filter: &str, offset: u64) -> AppResult<Value> {
        let snapshot = self.snapshot()?;
        if !["all", "open", "closed", "scheduled", "unscheduled"].contains(&filter) {
            return Err(AppError::Message("invalid issue filter".to_owned()));
        }
        let offset = usize::try_from(offset).unwrap_or(usize::MAX);
        let matching = snapshot
            .issues
            .iter()
            .rev()
            .filter(|issue| match filter {
                "open" => issue.status == IssueStatus::Open,
                "closed" => issue.status == IssueStatus::Closed,
                "scheduled" => issue.status == IssueStatus::Open && issue.task_id != 0,
                "unscheduled" => issue.status == IssueStatus::Open && issue.task_id == 0,
                _ => true,
            })
            .collect::<Vec<_>>();
        let total = matching.len();
        let issues = matching
            .into_iter()
            .skip(offset)
            .take(SNAPSHOT_ISSUE_LIMIT)
            .map(|issue| issue_detail_summary(&snapshot, issue))
            .collect::<Vec<_>>();
        Ok(json!({
            "generation": self.generation,
            "issues": issues,
            "offset": offset,
            "bounds": bound(issues.len(), total),
        }))
    }

    fn move_issue_task_v1(
        &self,
        issue_id: u64,
        expected_task_id: u64,
        expected_plan_id: u64,
        plan_id: u64,
    ) -> AppResult<Value> {
        let _transition = lock(&self.resource_transition);
        let _admission = self.fence_resource_admission()?;
        if lock(&self.resource_admission.state).pending != 0 {
            return Err(message(
                "issue move must retry after resource admission completes",
            ));
        }
        self.with_exact_task_resources(&self.snapshot()?, expected_task_id, |resources| {
            if !resources.is_empty() {
                return Err(message(
                    "stop or detach linked terminals and agents before moving this task to another plan",
                ));
            }
            let result = lock(&self.application).mutate(Mutation::MoveIssueTask {
                id: issue_id,
                expected_task_id,
                expected_plan_id,
                plan_id,
            })?;
            issue_mutation_value(self.generation, result, "issue move")
        })
    }

    fn schedule_issue_v1(&self, issue_id: u64, plan_id: u64, title: &str) -> AppResult<Value> {
        let requested_title = title.trim();
        let task_title = if requested_title.is_empty() {
            self.project_store()?.issue(issue_id)?.title
        } else {
            requested_title.to_owned()
        };
        let result = lock(&self.application).mutate(Mutation::ScheduleIssue {
            id: issue_id,
            plan_id,
            task_title,
        })?;
        let MutationResult::ScheduledIssue { issue, task } = result else {
            return Err(unavailable("issue scheduling mutation"));
        };
        Ok(json!({
            "generation": self.generation,
            "issue": issue_record_value(&issue),
            "task": task_card(&self.snapshot()?, &task),
        }))
    }
}

/// The one-issue answer every issue mutation returns.
fn issue_mutation_value(
    generation: u64,
    result: MutationResult,
    mutation: &str,
) -> AppResult<Value> {
    let MutationResult::Issue(issue) = result else {
        return Err(unavailable(mutation));
    };
    Ok(json!({ "generation": generation, "issue": issue_record_value(&issue) }))
}

fn parse_issue_status(value: &str) -> AppResult<IssueStatus> {
    IssueStatus::from_name(value)
        .ok_or_else(|| AppError::Message(format!("invalid issue status {value:?}")))
}

fn parse_issue_severity(value: &str) -> AppResult<Severity> {
    Severity::from_name(value)
        .ok_or_else(|| AppError::Message(format!("invalid issue severity {value:?}")))
}

fn parse_issue_timestamp(value: &str) -> AppResult<Timestamp> {
    if value == "0001-01-01T00:00:00Z" {
        return Ok(Timestamp::Zero);
    }
    let parsed = OffsetDateTime::parse(value, &Rfc3339)
        .map_err(|_| AppError::Message("issue timestamp is invalid".to_owned()))?;
    Ok(Timestamp::Fixed {
        seconds: parsed.unix_timestamp(),
        nanoseconds: parsed.nanosecond(),
        offset_seconds: parsed.offset().whole_seconds(),
    })
}
