//! Task commands: first-run tasks, board mutations, the confirmed status
//! move, and the task detail drawer.

use std::time::Instant;

use ptrack_core::{NoteTarget, ProjectSnapshot, Task, TaskStatus, Timestamp};
use ptrack_store::StoreError;
use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::super::command::TaskCommand;
use super::super::ports::DesktopWorkspace;
use super::super::projection::{
    agent_intelligence_for_task_result, task_card, task_detail_value, task_linked_runtime,
};
use super::super::support::{
    first_run_title, first_task_view, lock, message, parse_first_run_timestamp, parse_task_status,
    random_token, timestamp, trimmed_nonempty, unavailable, value,
};
use super::super::wire::CreateFirstTaskResultV1;
use super::super::{TASK_CONFIRMATION_LIMIT, TASK_CONFIRMATION_TTL};
use super::BoundDesktopWorkspace;
use super::resources::TaskResource;
use crate::{AppError, AppResult, Mutation, MutationResult};

#[derive(Clone)]
pub(super) struct TaskChallenge {
    pub(super) generation: u64,
    pub(super) task_id: u64,
    pub(super) plan_id: u64,
    pub(super) from_status: TaskStatus,
    pub(super) to_status: TaskStatus,
    pub(super) task_updated_at: Timestamp,
    pub(super) terminal_revision: u64,
    pub(super) agent_revision: u64,
    pub(super) admission_revision: u64,
    pub(super) resources: Vec<TaskResource>,
    pub(super) active_terminals: usize,
    pub(super) active_agents: usize,
    pub(super) issued_at: Instant,
    pub(super) expires_at: Instant,
}

impl BoundDesktopWorkspace {
    pub(super) fn task_command(&self, command: TaskCommand<'_>) -> AppResult<Value> {
        match command {
            TaskCommand::CreateFirst {
                generation,
                plan_id,
                title,
            } => {
                self.require_exact_generation(generation)?;
                let title = first_run_title(title, "task")?;
                let task = self.project_store()?.create_first_task(plan_id, title)?;
                value(CreateFirstTaskResultV1 {
                    task: first_task_view(&task),
                    state: self.first_run_state(),
                })
            }
            TaskCommand::StartFirst {
                generation,
                task_id,
                expected_updated_at,
            } => {
                self.require_exact_generation(generation)?;
                let task = self.start_first_task_v1(task_id, expected_updated_at)?;
                value(CreateFirstTaskResultV1 {
                    task: first_task_view(&task),
                    state: self.first_run_state(),
                })
            }
            TaskCommand::Add {
                generation,
                plan_id,
                title,
            } => {
                self.require_exact_generation(generation)?;
                let title = trimmed_nonempty(title, "task title cannot be empty")?;
                let result =
                    lock(&self.application).mutate(Mutation::AddTask { plan_id, title })?;
                let MutationResult::Task(task) = result else {
                    return Err(unavailable("task mutation"));
                };
                let card = task_card(&self.snapshot()?, &task);
                Ok(json!({ "generation": self.generation, "task": card }))
            }
            TaskCommand::Rename {
                generation,
                task_id,
                title,
            } => {
                self.require_exact_generation(generation)?;
                let title = trimmed_nonempty(title, "task title cannot be empty")?;
                lock(&self.application).mutate(Mutation::SetTaskTitle { id: task_id, title })?;
                Ok(json!({ "generation": self.generation }))
            }
            TaskCommand::AddNote {
                generation,
                task_id,
                body,
            } => {
                self.require_exact_generation(generation)?;
                let body = trimmed_nonempty(body, "memory note cannot be empty")?;
                if self.snapshot()?.task(task_id).is_none() {
                    return Err(AppError::Message(format!("task #{task_id} not found")));
                }
                lock(&self.application).mutate(Mutation::AddNote {
                    target: NoteTarget::Task,
                    target_id: task_id,
                    body,
                })?;
                Ok(json!({ "generation": self.generation }))
            }
            TaskCommand::Move {
                generation,
                task_id,
                status,
                confirmation_token,
            } => {
                self.require_exact_generation(generation)?;
                let status = parse_task_status(status)?;
                self.move_task_v3(task_id, status, confirmation_token)
            }
            TaskCommand::Detail {
                generation,
                task_id,
            } => {
                self.require_generation(generation)?;
                value(self.task_detail(&self.snapshot()?, task_id)?)
            }
        }
    }

    fn task_detail(&self, snapshot: &ProjectSnapshot, task_id: u64) -> AppResult<Value> {
        let projection = self.runtime_projection(snapshot)?;
        let detail = task_linked_runtime(&projection, task_id);
        let mut intelligence = Vec::new();
        if let Some(agent) = &self.agent {
            for run in &detail.agents {
                if let Some(value) = agent_intelligence_for_task_result(
                    run,
                    task_id,
                    agent.agent_intelligence(self.generation, &run.run_id),
                )? {
                    intelligence.push(value);
                }
            }
        }
        task_detail_value(self.generation, snapshot, task_id, &detail, &intelligence)
    }

    fn issue_task_challenge(&self, mut challenge: TaskChallenge) -> AppResult<(String, String)> {
        let now = Instant::now();
        let mut challenges = lock(&self.task_challenges);
        challenges.retain(|_, value| now < value.expires_at);
        if challenges.len() >= TASK_CONFIRMATION_LIMIT
            && let Some(oldest) = challenges
                .iter()
                .min_by(|(left_token, left), (right_token, right)| {
                    left.issued_at
                        .cmp(&right.issued_at)
                        .then_with(|| left_token.cmp(right_token))
                })
                .map(|(token, _)| token.clone())
        {
            challenges.remove(&oldest);
        }
        challenge.issued_at = now;
        challenge.expires_at = now + TASK_CONFIRMATION_TTL;
        let expires_at = (OffsetDateTime::now_utc() + time::Duration::seconds(90))
            .format(&Rfc3339)
            .map_err(message)?;
        for _ in 0..4 {
            let token = random_token()?;
            if challenges.contains_key(&token) {
                continue;
            }
            challenges.insert(token.clone(), challenge.clone());
            drop(challenges);
            return Ok((token, expires_at));
        }
        Err(AppError::Message(
            "create unique task transition confirmation".to_owned(),
        ))
    }

    fn consume_task_challenge(
        &self,
        token: &str,
        task_id: u64,
        to_status: TaskStatus,
    ) -> AppResult<TaskChallenge> {
        if token.is_empty() || token.len() > 128 {
            return Err(invalid_task_confirmation());
        }
        let now = Instant::now();
        let mut challenges = lock(&self.task_challenges);
        challenges.retain(|_, value| now < value.expires_at);
        let challenge = challenges
            .remove(token)
            .ok_or_else(invalid_task_confirmation)?;
        drop(challenges);
        if now >= challenge.expires_at
            || challenge.generation != self.generation
            || challenge.task_id != task_id
            || challenge.to_status != to_status
        {
            return Err(invalid_task_confirmation());
        }
        Ok(challenge)
    }

    #[allow(clippy::too_many_lines)]
    fn move_task_v3(
        &self,
        task_id: u64,
        wanted: TaskStatus,
        confirmation_token: &str,
    ) -> AppResult<Value> {
        let _transition = lock(&self.resource_transition);
        let _admission = self.fence_resource_admission()?;
        if lock(&self.resource_admission.state).pending != 0 {
            return Err(AppError::Message(
                "task transition must retry after resource admission completes".to_owned(),
            ));
        }
        let store = self.project_store()?;
        if confirmation_token.is_empty() {
            let task = store.task(task_id).map_err(|error| match error {
                StoreError::NotFound => AppError::Message(format!("task #{task_id} not found")),
                other => AppError::from(other),
            })?;
            let base = task_transition_base(self.generation, &task, wanted);
            if task.status == wanted {
                return Ok(task_transition_applied(base));
            }
            let revisions = self.resource_revisions()?;
            let snapshot = store.snapshot()?;
            return self.with_exact_task_resources(&snapshot, task_id, |resources| {
                let active_terminals = resources
                    .iter()
                    .filter(|resource| resource.kind == "terminal")
                    .count();
                let active_agents = resources.len().saturating_sub(active_terminals);
                if resources.is_empty() {
                    store
                        .compare_and_set_task_status_with_notes(
                            task.id,
                            task.plan_id,
                            task.status,
                            task.updated_at,
                            wanted,
                            &desktop_close_override(&snapshot, task.id, task.status, wanted),
                        )
                        .map_err(AppError::from)?;
                    return Ok(task_transition_applied(base));
                }
                let (token, expires_at) = self.issue_task_challenge(TaskChallenge {
                    generation: self.generation,
                    task_id,
                    plan_id: task.plan_id,
                    from_status: task.status,
                    to_status: wanted,
                    task_updated_at: task.updated_at,
                    terminal_revision: revisions.0,
                    agent_revision: revisions.1,
                    admission_revision: self.admission_revision(),
                    resources,
                    active_terminals,
                    active_agents,
                    issued_at: Instant::now(),
                    expires_at: Instant::now(),
                })?;
                Ok(json!({
                    "generation": self.generation,
                    "taskId": task_id,
                    "fromStatus": task.status.as_str(),
                    "toStatus": wanted.as_str(),
                    "applied": false,
                    "requiresConfirmation": true,
                    "confirmation": {
                        "token": token,
                        "expiresAt": expires_at,
                        "activeTerminals": active_terminals,
                        "activeAgents": active_agents
                    }
                }))
            });
        }

        let challenge = self.consume_task_challenge(confirmation_token, task_id, wanted)?;
        if self.resource_revisions()? != (challenge.terminal_revision, challenge.agent_revision)
            || self.admission_revision() != challenge.admission_revision
        {
            return Err(invalid_task_confirmation());
        }
        let snapshot = store.snapshot()?;
        self.with_exact_task_resources(&snapshot, task_id, |resources| {
            let active_terminals = resources
                .iter()
                .filter(|resource| resource.kind == "terminal")
                .count();
            let active_agents = resources.len().saturating_sub(active_terminals);
            if resources != challenge.resources
                || active_terminals != challenge.active_terminals
                || active_agents != challenge.active_agents
                || self.resource_revisions()?
                    != (challenge.terminal_revision, challenge.agent_revision)
                || self.admission_revision() != challenge.admission_revision
            {
                return Err(invalid_task_confirmation());
            }
            let result = store.compare_and_set_task_status_with_notes(
                task_id,
                challenge.plan_id,
                challenge.from_status,
                challenge.task_updated_at,
                wanted,
                &desktop_close_override(&snapshot, task_id, challenge.from_status, wanted),
            );
            match result {
                Ok(_) => Ok(task_transition_applied(json!({
                    "generation": self.generation,
                    "taskId": task_id,
                    "fromStatus": challenge.from_status.as_str(),
                    "toStatus": wanted.as_str()
                }))),
                Err(StoreError::TaskStatusChanged(_)) => Err(invalid_task_confirmation()),
                Err(error) => Err(AppError::from(error)),
            }
        })
    }

    fn start_first_task_v1(&self, task_id: u64, expected_updated_at: &str) -> AppResult<Task> {
        let expected = parse_first_run_timestamp(expected_updated_at)?;
        let store = self.project_store()?;
        let task = store.task(task_id).map_err(|error| match error {
            StoreError::NotFound => AppError::Message(format!("task #{task_id} not found")),
            other => AppError::from(other),
        })?;
        if task.status == TaskStatus::Doing {
            return store
                .start_first_task(task_id, expected)
                .map_err(AppError::from);
        }
        let _transition = lock(&self.resource_transition);
        let _admission = self.fence_resource_admission()?;
        if lock(&self.resource_admission.state).pending != 0 {
            return Err(AppError::Message(
                "first task start must retry after resource admission completes".to_owned(),
            ));
        }
        if task.status != TaskStatus::Todo || timestamp(task.updated_at) != expected_updated_at {
            return Err(AppError::Message(
                "first task changed before it could be started".to_owned(),
            ));
        }
        let snapshot = store.snapshot()?;
        self.with_exact_task_resources(&snapshot, task_id, |resources| {
            if !resources.is_empty() {
                return Err(AppError::Message(
                    "first task start requires resource confirmation".to_owned(),
                ));
            }
            store
                .start_first_task(task_id, expected)
                .map_err(AppError::from)
        })
    }
}

/// The override note a board move to Done records when the task has no
/// closeout summary or no linked commit. Humans may close without evidence,
/// but never silently: the note lands in the same transaction as the status,
/// in the exact form `close_task_from_ui` writes for the desktop surface.
fn desktop_close_override(
    snapshot: &ProjectSnapshot,
    task_id: u64,
    from: TaskStatus,
    to: TaskStatus,
) -> Vec<String> {
    if to != TaskStatus::Done || from == TaskStatus::Done {
        return Vec::new();
    }
    crate::service::ui_close_override_notes(snapshot, crate::UiSurface::Desktop, task_id)
}

fn task_transition_base(generation: u64, task: &Task, wanted: TaskStatus) -> Value {
    json!({
        "generation": generation,
        "taskId": task.id,
        "fromStatus": task.status.as_str(),
        "toStatus": wanted.as_str()
    })
}

fn task_transition_applied(mut value: Value) -> Value {
    if let Some(object) = value.as_object_mut() {
        object.insert("applied".to_owned(), Value::Bool(true));
        object.insert("requiresConfirmation".to_owned(), Value::Bool(false));
    }
    value
}

fn invalid_task_confirmation() -> AppError {
    AppError::Message("task transition confirmation is invalid or stale".to_owned())
}
