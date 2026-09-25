//! Terminal commands: profiles, session lifecycle, stream tickets, the
//! plan/task association, and the reviewed memory write-back.

use std::path::Path;

use ptrack_core::{MemoryKind, NoteTarget, ProjectSnapshot};
use ptrack_store::MemoryWriteRequest;
use ptrack_terminal::TerminalAssociationPointer;
use serde_json::{Value, json};

use super::super::args::missing_arg;
use super::super::command::TerminalCommand;
use super::super::support::{lock, message, unavailable, value};
use super::BoundDesktopWorkspace;
use super::resources::agent_association;
use crate::{AppError, AppResult};
use ptrack_agent::{AssociationPointer as AgentAssociationPointer, contains_potential_credential};

impl BoundDesktopWorkspace {
    pub(super) fn terminal_command(&self, command: TerminalCommand<'_>) -> AppResult<Value> {
        match command {
            TerminalCommand::Profiles { generation } => self.terminal_profiles(generation),
            TerminalCommand::ValidateCwds { generation, cwds } => {
                self.require_exact_generation(generation)?;
                value(
                    self.terminal
                        .as_ref()
                        .ok_or_else(|| unavailable("terminal manager"))?
                        .validate_cwds(self.generation, &cwds)?,
                )
            }
            TerminalCommand::Create {
                generation,
                profile_id,
                cwd,
                rows,
                columns,
            } => {
                self.require_exact_generation(generation)?;
                self.create_terminal(profile_id, cwd, rows, columns)
            }
            TerminalCommand::Resize {
                generation,
                session_id,
                rows,
                columns,
            } => {
                self.require_exact_generation(generation)?;
                self.resize_terminal(session_id, rows, columns)
            }
            // Fenced by the bound workspace generation, so a ticket can never
            // be minted for a session belonging to a superseded project.
            TerminalCommand::ClaimStream {
                session_id,
                from_sequence,
            } => value(
                self.terminal
                    .as_ref()
                    .ok_or_else(|| unavailable("terminal manager"))?
                    .claim_stream_ticket(self.generation, session_id, from_sequence)?,
            ),
            TerminalCommand::Close {
                generation,
                session_id,
                force,
            } => {
                self.require_exact_generation(generation)?;
                self.terminal
                    .as_ref()
                    .ok_or_else(|| unavailable("terminal manager"))?
                    .close(self.generation, session_id, force)?;
                Ok(json!({ "generation": self.generation }))
            }
            TerminalCommand::MutateAssociation {
                generation,
                session_id,
                expected_revision,
                detach,
                pointer,
            } => {
                self.require_exact_generation(generation)?;
                self.mutate_terminal_association(session_id, expected_revision, detach, pointer)
            }
            TerminalCommand::PreviewWriteback {
                generation,
                session_id,
                revision,
                kind,
                content,
            } => {
                self.require_exact_generation(generation)?;
                self.preview_terminal_writeback(session_id, revision, kind, content)
            }
            TerminalCommand::WriteMemory {
                generation,
                session_id,
                revision,
                request_id,
                kind,
                content,
                confirm_summary,
            } => {
                self.require_exact_generation(generation)?;
                let kind = memory_kind(kind)?;
                let content = validate_writeback_content(content)?;
                if kind == MemoryKind::Summary && !confirm_summary.ok_or_else(|| missing_arg(6))? {
                    return Err(AppError::Message(
                        "summary replacement requires explicit confirmation".to_owned(),
                    ));
                }
                self.write_terminal_memory(session_id, revision, request_id, kind, content)
            }
        }
    }

    /// `GetTerminalProfiles` answers the bare list with no fence;
    /// `GetTerminalProfilesV2` is fenced and answers the whole document.
    fn terminal_profiles(&self, generation: Option<u64>) -> AppResult<Value> {
        let terminal = self
            .terminal
            .as_ref()
            .ok_or_else(|| unavailable("terminal manager"))?;
        if let Some(generation) = generation {
            self.require_exact_generation(generation)?;
        }
        let profiles = terminal.profiles(self.generation)?;
        if generation.is_some() {
            value(profiles)
        } else {
            value(profiles.profiles)
        }
    }

    fn create_terminal(
        &self,
        profile_id: &str,
        cwd: &str,
        rows: u16,
        columns: u16,
    ) -> AppResult<Value> {
        let _admission = self.begin_resource_admission()?;
        let _transition = lock(&self.resource_transition);
        let cwd = if cwd.is_empty() {
            None
        } else {
            Some(Path::new(cwd))
        };
        let result = self
            .terminal
            .as_ref()
            .ok_or_else(|| unavailable("terminal manager"))?
            .create(self.generation, profile_id, cwd, rows, columns)?;
        value(result)
    }

    fn resize_terminal(&self, session_id: &str, rows: u16, columns: u16) -> AppResult<Value> {
        self.terminal
            .as_ref()
            .ok_or_else(|| unavailable("terminal manager"))?
            .resize(
                self.generation,
                session_id,
                // This caller has no renderer lease; the manager uses the
                // current live lease for the resize.
                None,
                rows,
                columns,
            )?;
        Ok(json!({ "generation": self.generation }))
    }

    fn mutate_terminal_association(
        &self,
        session_id: &str,
        expected_revision: u64,
        detach: bool,
        pointer: TerminalAssociationPointer,
    ) -> AppResult<Value> {
        if !detach && pointer.plan_id == 0 {
            return Err(AppError::Message(
                "invalid association target: relink requires a plan or task".to_owned(),
            ));
        }
        validate_association_pointer(&self.snapshot()?, pointer)?;
        let _transition = lock(&self.resource_transition);
        let terminal = self
            .terminal
            .as_ref()
            .ok_or_else(|| unavailable("terminal association manager"))?;
        let change = terminal.prepare_association_change(
            self.generation,
            session_id,
            expected_revision,
            pointer,
        )?;
        if detach
            && change
                .previous
                .as_ref()
                .is_none_or(|value| value.pointer.plan_id == 0)
        {
            return Err(AppError::Message(
                "invalid association target: terminal is already detached".to_owned(),
            ));
        }
        let previous = change.previous.as_ref().map(|association| {
            agent_association(
                &self.endpoint.root,
                self.generation,
                session_id,
                association,
            )
        });
        let next = agent_association(
            &self.endpoint.root,
            self.generation,
            session_id,
            &change.next,
        );
        let agent_change = if let Some(agent) = &self.agent {
            agent.prepare_linked_association(
                self.generation,
                session_id,
                previous.as_ref(),
                &next,
                AgentAssociationPointer {
                    version: pointer.version,
                    plan_id: pointer.plan_id,
                    task_id: pointer.task_id,
                },
            )?
        } else {
            None
        };
        let _event_suppression = self
            .agent
            .as_ref()
            .map(|agent| agent.suppress_runtime_event(self.generation))
            .transpose()?;
        terminal.commit_association_change(self.generation, &change)?;
        if let Some(agent_change) = &agent_change
            && let Some(agent) = &self.agent
            && let Err(error) = agent.commit_linked_association(self.generation, agent_change)
        {
            let rollback = terminal.rollback_association_change(self.generation, &change);
            return Err(AppError::Message(match rollback {
                Ok(()) => error.to_string(),
                Err(rollback) => format!("{error}\n{rollback}"),
            }));
        }
        terminal.association_changed(self.generation)?;
        let mut result = json!({
            "generation": self.generation,
            "sessionId": session_id,
            "revision": change.next.revision,
            "detached": detach
        });
        if !detach {
            result["pointer"] = serde_json::to_value(pointer).map_err(message)?;
        }
        Ok(result)
    }

    fn preview_terminal_writeback(
        &self,
        session_id: &str,
        revision: u64,
        kind: &str,
        content: &str,
    ) -> AppResult<Value> {
        let kind = memory_kind(kind)?;
        let content = validate_writeback_content(content)?;
        let association = self
            .terminal
            .as_ref()
            .ok_or_else(|| unavailable("terminal write-back"))?
            .live_association(self.generation, session_id, revision)?;
        let destination = writeback_destination(&self.snapshot()?, association.pointer, kind)?;
        Ok(json!({
            "generation": self.generation,
            "sessionId": session_id,
            "revision": association.revision,
            "kind": kind.as_str(),
            "content": content,
            "contentBytes": content.len(),
            "associationTarget": writeback_target_label(association.pointer),
            "destination": destination,
            "replacesSummary": kind == MemoryKind::Summary
        }))
    }

    fn write_terminal_memory(
        &self,
        session_id: &str,
        revision: u64,
        request_id: &str,
        kind: MemoryKind,
        content: String,
    ) -> AppResult<Value> {
        let association = self
            .terminal
            .as_ref()
            .ok_or_else(|| unavailable("terminal write-back"))?
            .live_association(self.generation, session_id, revision)?;
        let snapshot = self.snapshot()?;
        let destination = writeback_destination(&snapshot, association.pointer, kind)?;
        let (target, target_id, plan_id) = writeback_target(association.pointer, kind);
        let store = self.project_store()?;
        let result = store.write_memory(MemoryWriteRequest {
            request_id: request_id.to_owned(),
            kind,
            body: content,
            target,
            target_id,
            plan_id,
            workspace_generation: self.generation,
            session_id: session_id.to_owned(),
            association_revision: association.revision,
        })?;
        Ok(json!({
            "generation": self.generation,
            "sessionId": session_id,
            "revision": association.revision,
            "requestId": request_id,
            "kind": kind.as_str(),
            "destination": destination,
            "noteId": result.note.as_ref().map_or(0, |note| note.id),
            "replayed": result.replayed
        }))
    }
}

pub(super) fn validate_association_pointer(
    snapshot: &ProjectSnapshot,
    pointer: TerminalAssociationPointer,
) -> AppResult<()> {
    if pointer.version != 1 {
        return Err(AppError::Message(
            "invalid association target: unsupported pointer version".to_owned(),
        ));
    }
    if pointer.plan_id == 0 {
        if pointer.task_id != 0 {
            return Err(AppError::Message(
                "invalid association target: task requires a plan".to_owned(),
            ));
        }
        return Ok(());
    }
    if snapshot.plan(pointer.plan_id).is_none() {
        return Err(AppError::Message(format!(
            "invalid association target: plan #{} not found",
            pointer.plan_id
        )));
    }
    if pointer.task_id != 0
        && snapshot
            .task(pointer.task_id)
            .is_none_or(|task| task.plan_id != pointer.plan_id)
    {
        return Err(AppError::Message(format!(
            "invalid association target: task #{} is not in plan #{}",
            pointer.task_id, pointer.plan_id
        )));
    }
    Ok(())
}

/// A write-back memory kind: one of the reviewed kinds a terminal may write.
fn memory_kind(raw: &str) -> AppResult<MemoryKind> {
    let kind = MemoryKind::from_name(raw).ok_or_else(|| {
        AppError::Message("write-back content is invalid: unsupported type".to_owned())
    })?;
    if matches!(
        kind,
        MemoryKind::Decision | MemoryKind::Blocker | MemoryKind::Handoff | MemoryKind::Summary
    ) {
        Ok(kind)
    } else {
        Err(AppError::Message(
            "write-back content is invalid: unsupported type".to_owned(),
        ))
    }
}

fn validate_writeback_content(raw: &str) -> AppResult<String> {
    let normalized = raw.replace("\r\n", "\n").replace('\r', "\n");
    let normalized = normalized.trim().to_owned();
    if normalized.is_empty() {
        return Err(AppError::Message(
            "write-back content is invalid: content is required".to_owned(),
        ));
    }
    if normalized.len() > 8 * 1024
        || normalized.chars().count() > 4_000
        || normalized.lines().count() > 128
    {
        return Err(AppError::Message(
            "write-back content is invalid: content exceeds the hard limit".to_owned(),
        ));
    }
    if normalized
        .chars()
        .any(|value| value.is_control() && value != '\n' && value != '\t')
    {
        return Err(AppError::Message(
            "write-back content is invalid: content contains unsupported characters".to_owned(),
        ));
    }
    if contains_potential_credential(&normalized) {
        return Err(AppError::Message(
            "write-back content may contain a credential".to_owned(),
        ));
    }
    Ok(normalized)
}

fn writeback_target_label(pointer: TerminalAssociationPointer) -> String {
    if pointer.plan_id == 0 {
        "Project".to_owned()
    } else if pointer.task_id == 0 {
        format!("Plan #{}", pointer.plan_id)
    } else {
        format!("Task #{}", pointer.task_id)
    }
}

fn writeback_destination(
    snapshot: &ProjectSnapshot,
    pointer: TerminalAssociationPointer,
    kind: MemoryKind,
) -> AppResult<String> {
    validate_association_pointer(snapshot, pointer)?;
    Ok(if kind == MemoryKind::Summary {
        "Project rolling summary".to_owned()
    } else {
        writeback_target_label(pointer)
    })
}

fn writeback_target(
    pointer: TerminalAssociationPointer,
    kind: MemoryKind,
) -> (NoteTarget, u64, u64) {
    if kind == MemoryKind::Summary || pointer.plan_id == 0 {
        (NoteTarget::Project, 0, 0)
    } else if pointer.task_id == 0 {
        (NoteTarget::Plan, pointer.plan_id, pointer.plan_id)
    } else {
        (NoteTarget::Task, pointer.task_id, pointer.plan_id)
    }
}
