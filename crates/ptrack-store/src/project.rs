use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use ptrack_capability_policy::{ApprovalProof, SanitizedAudit, normalize};
use ptrack_core::{
    CAPABILITY_MODEL_VERSION, Capability, CapabilityAudit, Commit, Counts, Digest32, Issue,
    IssueStatus, MemoryKind, MemoryWritebackRecord, Meta, Milestone, MilestoneStatus, Note,
    NoteTarget, Plan, PlanStatus, ProjectSnapshot, Severity, Task, TaskStatus, Timestamp,
};

use crate::typed::{self, StoredRecord};
use crate::{
    ActivatedStore, ActiveBinding, Clock, Collection, ReadTransaction, RecordKey, StagedStore,
    Store, StoreError, StoreKind, StoreResult, SystemClock, WriteTransaction,
};

pub const CURRENT_PROJECT_FORMAT: u64 = 5;
pub const MEMORY_WRITEBACK_REPLAY_LIMIT: usize = 256;
pub const CAPABILITY_AUDIT_GLOBAL_LIMIT: i64 = 5_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryWriteRequest {
    pub request_id: String,
    pub kind: MemoryKind,
    pub body: String,
    pub target: NoteTarget,
    pub target_id: u64,
    pub plan_id: u64,
    pub workspace_generation: u64,
    pub session_id: String,
    pub association_revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryWriteResult {
    pub note: Option<Note>,
    pub summary: String,
    pub replayed: bool,
}

/// Typed, activation-bound project storage.
pub struct ProjectStore {
    active: ActivatedStore,
    clock: Arc<dyn Clock>,
    writer_version: String,
}

impl ProjectStore {
    pub fn create_new(
        path: impl AsRef<Path>,
        binding: ActiveBinding,
        writer_version: impl Into<String>,
    ) -> StoreResult<Self> {
        Self::create_new_with_clock(path, binding, writer_version, SystemClock)
    }

    pub fn create_new_with_clock(
        path: impl AsRef<Path>,
        binding: ActiveBinding,
        writer_version: impl Into<String>,
        clock: impl Clock + 'static,
    ) -> StoreResult<Self> {
        if binding.kind != StoreKind::Project {
            return Err(StoreError::ActivationBinding(
                "project store requires project binding".to_owned(),
            ));
        }
        let store = Store::create_new(path, StoreKind::Project)?;
        store.activate(&binding)?;
        let project = Self {
            active: ActivatedStore { store, binding },
            clock: Arc::new(clock),
            writer_version: writer_version.into(),
        };
        let now = project.clock.now_local();
        let writer = project.writer_version.clone();
        project.write(|transaction| {
            typed::put(
                transaction,
                RecordKey::Singleton,
                &Meta {
                    goal: String::new(),
                    summary: String::new(),
                    active_plan: 0,
                    created_at: now,
                    updated_at: now,
                    format_version: CURRENT_PROJECT_FORMAT,
                    last_write_version: writer,
                },
            )?;
            Ok(())
        })?;
        Ok(project)
    }

    pub fn activate(
        staged: StagedStore,
        binding: ActiveBinding,
        writer_version: impl Into<String>,
    ) -> StoreResult<Self> {
        Self::from_activated(staged.activate(binding)?, writer_version, SystemClock)
    }

    pub fn open_existing(
        path: impl AsRef<Path>,
        binding: &ActiveBinding,
        writer_version: impl Into<String>,
    ) -> StoreResult<Self> {
        Self::from_activated(
            ActivatedStore::open(path, binding)?,
            writer_version,
            SystemClock,
        )
    }

    pub fn from_activated(
        active: ActivatedStore,
        writer_version: impl Into<String>,
        clock: impl Clock + 'static,
    ) -> StoreResult<Self> {
        if active.binding.kind != StoreKind::Project {
            return Err(StoreError::ActivationBinding(
                "project store requires project binding".to_owned(),
            ));
        }
        let project = Self {
            active,
            clock: Arc::new(clock),
            writer_version: writer_version.into(),
        };
        project.migrate_legacy_meta()?;
        Ok(project)
    }

    #[must_use]
    pub fn binding(&self) -> &ActiveBinding {
        self.active.binding()
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        self.active.store.path()
    }

    pub fn application_writes(&self) -> StoreResult<bool> {
        self.active.application_writes()
    }

    pub(crate) fn write<R>(
        &self,
        operation: impl FnOnce(&mut WriteTransaction) -> StoreResult<R>,
    ) -> StoreResult<R> {
        self.active.write(operation)
    }

    pub(crate) fn read<R>(
        &self,
        operation: impl FnOnce(&ReadTransaction) -> StoreResult<R>,
    ) -> StoreResult<R> {
        self.active.store.read(operation)
    }

    pub(crate) fn raw_writer_barrier<R>(
        &self,
        operation: impl FnOnce(&Path) -> StoreResult<R>,
    ) -> StoreResult<R> {
        self.active.store.with_writer_barrier(operation)
    }

    pub(crate) fn json_stage_provenance(&self) -> StoreResult<Option<crate::JsonStageProvenance>> {
        self.active.store.json_stage_provenance()
    }

    pub fn meta(&self) -> StoreResult<Meta> {
        self.active.store.read(|transaction| {
            typed::get(transaction, RecordKey::Singleton)?.ok_or(StoreError::NotFound)
        })
    }

    pub fn set_goal(&self, goal: impl Into<String>) -> StoreResult<()> {
        let goal = goal.into();
        self.update_meta(|meta| meta.goal = goal)
    }

    pub fn set_summary(&self, summary: impl Into<String>) -> StoreResult<()> {
        let summary = summary.into();
        self.update_meta(|meta| meta.summary = summary)
    }

    pub fn set_active_plan(&self, plan_id: u64) -> StoreResult<()> {
        let now = self.clock.now_local();
        let writer = self.writer_version.clone();
        self.write(|transaction| {
            if plan_id != 0
                && typed::get_write::<Plan>(transaction, RecordKey::Id(plan_id))?.is_none()
            {
                return Err(StoreError::NotFound);
            }
            let mut meta = required_write::<Meta>(transaction, RecordKey::Singleton)?;
            meta.active_plan = plan_id;
            stamp_meta(&mut meta, now, writer);
            typed::put(transaction, RecordKey::Singleton, &meta)?;
            Ok(())
        })
    }

    fn update_meta(&self, mutate: impl FnOnce(&mut Meta)) -> StoreResult<()> {
        let now = self.clock.now_local();
        let writer = self.writer_version.clone();
        self.write(|transaction| {
            let mut meta = required_write::<Meta>(transaction, RecordKey::Singleton)?;
            mutate(&mut meta);
            stamp_meta(&mut meta, now, writer);
            typed::put(transaction, RecordKey::Singleton, &meta)?;
            Ok(())
        })
    }

    pub fn add_milestone(&self, title: impl Into<String>) -> StoreResult<Milestone> {
        let title = title.into();
        let now = self.clock.now_local();
        self.write(|transaction| {
            let order = count_write::<Milestone>(transaction)?;
            let id = transaction.next_id(Collection::Milestones)?;
            let value = Milestone {
                id,
                title,
                status: MilestoneStatus::Open,
                due: Timestamp::Zero,
                order,
                created_at: now,
                updated_at: now,
            };
            typed::put(transaction, RecordKey::Id(id), &value)?;
            Ok(value)
        })
    }

    pub fn milestones(&self) -> StoreResult<Vec<Milestone>> {
        self.list_ordered::<Milestone>()
    }

    pub fn milestone(&self, id: u64) -> StoreResult<Milestone> {
        self.get_id(id)
    }

    pub fn set_milestone_status(&self, id: u64, status: MilestoneStatus) -> StoreResult<()> {
        self.mutate_id::<Milestone>(id, |value, now| {
            value.status = status;
            value.updated_at = now;
        })
    }

    pub fn set_milestone_title(&self, id: u64, title: impl Into<String>) -> StoreResult<()> {
        let title = title.into();
        self.mutate_id::<Milestone>(id, |value, now| {
            value.title = title;
            value.updated_at = now;
        })
    }

    pub fn set_milestone_due(&self, id: u64, due: Timestamp) -> StoreResult<()> {
        self.mutate_id::<Milestone>(id, |value, now| {
            value.due = due;
            value.updated_at = now;
        })
    }

    pub fn add_plan(&self, title: impl Into<String>, milestone_id: u64) -> StoreResult<Plan> {
        let title = title.into();
        let now = self.clock.now_local();
        self.write(|transaction| {
            if milestone_id != 0
                && typed::get_write::<Milestone>(transaction, RecordKey::Id(milestone_id))?
                    .is_none()
            {
                return Err(StoreError::NotFound);
            }
            let order = count_write::<Plan>(transaction)?;
            let id = transaction.next_id(Collection::Plans)?;
            let value = Plan {
                id,
                title,
                status: PlanStatus::Active,
                milestone_id,
                order,
                created_at: now,
                updated_at: now,
            };
            typed::put(transaction, RecordKey::Id(id), &value)?;
            Ok(value)
        })
    }

    pub fn plans(&self) -> StoreResult<Vec<Plan>> {
        self.list_ordered::<Plan>()
    }

    pub fn plan(&self, id: u64) -> StoreResult<Plan> {
        self.get_id(id)
    }

    pub fn set_plan_status(&self, id: u64, status: PlanStatus) -> StoreResult<()> {
        self.mutate_id::<Plan>(id, |value, now| {
            value.status = status;
            value.updated_at = now;
        })
    }

    pub fn set_plan_title(&self, id: u64, title: impl Into<String>) -> StoreResult<()> {
        let title = title.into();
        self.mutate_id::<Plan>(id, |value, now| {
            value.title = title;
            value.updated_at = now;
        })
    }

    pub fn set_plan_milestone(&self, id: u64, milestone_id: u64) -> StoreResult<()> {
        let now = self.clock.now_local();
        self.write(|transaction| {
            if milestone_id != 0
                && typed::get_write::<Milestone>(transaction, RecordKey::Id(milestone_id))?
                    .is_none()
            {
                return Err(StoreError::NotFound);
            }
            let mut plan = required_write::<Plan>(transaction, RecordKey::Id(id))?;
            plan.milestone_id = milestone_id;
            plan.updated_at = now;
            typed::put(transaction, RecordKey::Id(id), &plan)?;
            Ok(())
        })
    }

    pub fn add_task(&self, plan_id: u64, title: impl Into<String>) -> StoreResult<Task> {
        let title = title.into();
        let now = self.clock.now_local();
        self.write(|transaction| {
            require_id_write::<Plan>(transaction, plan_id)?;
            let order = count_write::<Task>(transaction)?;
            let id = transaction.next_id(Collection::Tasks)?;
            let value = Task {
                id,
                plan_id,
                title,
                status: TaskStatus::Todo,
                order,
                created_at: now,
                updated_at: now,
            };
            typed::put(transaction, RecordKey::Id(id), &value)?;
            Ok(value)
        })
    }

    pub fn tasks(&self) -> StoreResult<Vec<Task>> {
        self.list_ordered::<Task>()
    }

    pub fn task(&self, id: u64) -> StoreResult<Task> {
        self.get_id(id)
    }

    pub fn set_task_status(&self, id: u64, status: TaskStatus) -> StoreResult<()> {
        self.mutate_id::<Task>(id, |value, now| {
            value.status = status;
            value.updated_at = now;
        })
    }

    pub fn compare_and_set_task_status(
        &self,
        id: u64,
        expected_plan_id: u64,
        expected_status: TaskStatus,
        expected_updated_at: Timestamp,
        status: TaskStatus,
    ) -> StoreResult<Task> {
        let now = self.clock.now_local();
        self.write(|transaction| {
            let mut task = required_write::<Task>(transaction, RecordKey::Id(id))?;
            if task.plan_id != expected_plan_id
                || task.status != expected_status
                || !same_instant(task.updated_at, expected_updated_at)
            {
                return Err(StoreError::TaskStatusChanged(format!(
                    "task #{id} is plan #{}/\"{}\" at {}, expected plan #{}/\"{}\" at {}",
                    task.plan_id,
                    task.status.as_str(),
                    format_timestamp_utc(task.updated_at),
                    expected_plan_id,
                    expected_status.as_str(),
                    format_timestamp_utc(expected_updated_at)
                )));
            }
            if task.status != status {
                task.status = status;
                task.updated_at = now;
                typed::put(transaction, RecordKey::Id(id), &task)?;
            }
            Ok(task)
        })
    }

    pub fn set_task_title(&self, id: u64, title: impl Into<String>) -> StoreResult<()> {
        let title = title.into();
        self.mutate_id::<Task>(id, |value, now| {
            value.title = title;
            value.updated_at = now;
        })
    }

    pub fn set_task_plan(&self, id: u64, plan_id: u64) -> StoreResult<()> {
        let now = self.clock.now_local();
        self.write(|transaction| {
            require_id_write::<Plan>(transaction, plan_id)?;
            let mut task = required_write::<Task>(transaction, RecordKey::Id(id))?;
            task.plan_id = plan_id;
            task.updated_at = now;
            typed::put(transaction, RecordKey::Id(id), &task)?;
            Ok(())
        })
    }

    pub fn convert_task_to_plan(&self, id: u64) -> StoreResult<Plan> {
        let now = self.clock.now_local();
        self.write(|transaction| {
            let task = required_write::<Task>(transaction, RecordKey::Id(id))?;
            let parent = required_write::<Plan>(transaction, RecordKey::Id(task.plan_id))?;
            let order = count_write::<Plan>(transaction)?;
            let plan_id = transaction.next_id(Collection::Plans)?;
            let plan = Plan {
                id: plan_id,
                title: task.title,
                status: if task.status == TaskStatus::Done {
                    PlanStatus::Done
                } else {
                    PlanStatus::Active
                },
                milestone_id: parent.milestone_id,
                order,
                created_at: task.created_at,
                updated_at: now,
            };
            typed::put(transaction, RecordKey::Id(plan_id), &plan)?;
            for mut note in typed::scan_write::<Note>(transaction)? {
                if note.target == NoteTarget::Task && note.target_id == id {
                    note.target = NoteTarget::Plan;
                    note.target_id = plan_id;
                    typed::put(transaction, RecordKey::Id(note.id), &note)?;
                }
            }
            for mut commit in typed::scan_write::<Commit>(transaction)? {
                if commit.task_id == id {
                    commit.task_id = 0;
                    commit.plan_id = plan_id;
                    typed::put(transaction, RecordKey::Id(commit.id), &commit)?;
                }
            }
            for mut issue in typed::scan_write::<Issue>(transaction)? {
                if issue.task_id == id {
                    issue.task_id = 0;
                    issue.updated_at = now;
                    typed::put(transaction, RecordKey::Id(issue.id), &issue)?;
                }
            }
            transaction.delete(Collection::Tasks, RecordKey::Id(id))?;
            Ok(plan)
        })
    }

    pub fn add_note(
        &self,
        target: NoteTarget,
        target_id: u64,
        body: impl Into<String>,
    ) -> StoreResult<Note> {
        let body = body.into();
        let now = self.clock.now_local();
        self.write(|transaction| {
            let id = transaction.next_id(Collection::Notes)?;
            let note = Note {
                id,
                target,
                target_id,
                kind: MemoryKind::Legacy,
                body,
                created_at: now,
            };
            typed::put(transaction, RecordKey::Id(id), &note)?;
            Ok(note)
        })
    }

    pub fn notes(&self) -> StoreResult<Vec<Note>> {
        self.list::<Note>()
    }

    pub fn recent_notes(&self, limit: usize) -> StoreResult<Vec<Note>> {
        let mut values = self.notes()?;
        values.reverse();
        if limit > 0 {
            values.truncate(limit);
        }
        Ok(values)
    }

    pub fn add_issue(
        &self,
        title: impl Into<String>,
        body: impl Into<String>,
        severity: Option<Severity>,
        task_id: u64,
    ) -> StoreResult<Issue> {
        let title = title.into();
        let body = body.into();
        let now = self.clock.now_local();
        self.write(|transaction| {
            if task_id != 0 {
                require_id_write::<Task>(transaction, task_id)?;
            }
            let id = transaction.next_id(Collection::Issues)?;
            let issue = Issue {
                id,
                title,
                body,
                status: IssueStatus::Open,
                severity: severity.unwrap_or(Severity::Medium),
                task_id,
                created_at: now,
                updated_at: now,
            };
            typed::put(transaction, RecordKey::Id(id), &issue)?;
            Ok(issue)
        })
    }

    pub fn issues(&self) -> StoreResult<Vec<Issue>> {
        self.list::<Issue>()
    }

    pub fn issue(&self, id: u64) -> StoreResult<Issue> {
        self.get_id(id)
    }

    pub fn set_issue_status(&self, id: u64, status: IssueStatus) -> StoreResult<()> {
        self.mutate_id::<Issue>(id, |value, now| {
            value.status = status;
            value.updated_at = now;
        })
    }

    pub fn set_issue_severity(&self, id: u64, severity: Severity) -> StoreResult<()> {
        self.mutate_id::<Issue>(id, |value, now| {
            value.severity = severity;
            value.updated_at = now;
        })
    }

    pub fn set_issue_title(&self, id: u64, title: impl Into<String>) -> StoreResult<()> {
        let title = title.into();
        self.mutate_id::<Issue>(id, |value, now| {
            value.title = title;
            value.updated_at = now;
        })
    }

    pub fn add_commit(
        &self,
        sha: impl Into<String>,
        subject: impl Into<String>,
        plan_id: u64,
        task_id: u64,
    ) -> StoreResult<Commit> {
        let sha = sha.into();
        let subject = subject.into();
        let now = self.clock.now_local();
        self.write(|transaction| {
            if let Some(existing) = typed::scan_write::<Commit>(transaction)?
                .into_iter()
                .find(|commit| commit.sha == sha)
            {
                return Ok(existing);
            }
            let id = transaction.next_id(Collection::Commits)?;
            let commit = Commit {
                id,
                sha,
                subject,
                plan_id,
                task_id,
                created_at: now,
            };
            typed::put(transaction, RecordKey::Id(id), &commit)?;
            Ok(commit)
        })
    }

    pub fn commits(&self) -> StoreResult<Vec<Commit>> {
        self.list::<Commit>()
    }

    pub fn commits_by_task(&self, task_id: u64) -> StoreResult<Vec<Commit>> {
        let mut values = self.commits()?;
        values.reverse();
        values.retain(|commit| commit.task_id == task_id);
        Ok(values)
    }

    pub fn commits_by_plan(&self, plan_id: u64) -> StoreResult<Vec<Commit>> {
        let mut values = self.commits()?;
        values.reverse();
        values.retain(|commit| commit.plan_id == plan_id);
        Ok(values)
    }

    pub fn add_capability(&self, mut capability: Capability) -> StoreResult<Capability> {
        let now = self.clock.now_local();
        self.write(|transaction| {
            capability.id = transaction.next_id(Collection::Capabilities)?;
            if capability.model_version == 0 {
                capability.model_version = CAPABILITY_MODEL_VERSION;
            }
            capability.revision = 1;
            capability.enabled = false;
            capability.approved_at = Timestamp::Zero;
            capability.expires_at = Timestamp::Zero;
            capability.created_at = now;
            capability.updated_at = now;
            typed::put(transaction, RecordKey::Id(capability.id), &capability)?;
            Ok(capability)
        })
    }

    pub fn capability(&self, id: u64) -> StoreResult<Capability> {
        self.get_id(id)
    }

    pub fn capabilities(&self) -> StoreResult<Vec<Capability>> {
        self.list::<Capability>()
    }

    /// Replaces one draft using its revision as a compare-and-set fence.
    ///
    /// Caller-supplied lifecycle state is ignored. Material edits revoke the
    /// existing approval; name-only edits preserve it.
    pub fn update_capability(&self, mut capability: Capability) -> StoreResult<Capability> {
        let now = self.clock.now_local();
        self.write(|transaction| {
            let existing = required_write::<Capability>(transaction, RecordKey::Id(capability.id))?;
            require_capability_revision(capability.revision, existing.revision)?;
            let security_changed = capability_security_changed(&existing, &capability);
            capability.id = existing.id;
            capability.model_version = existing.model_version;
            capability.revision = existing.revision.checked_add(1).ok_or_else(|| {
                StoreError::InvalidManifest("capability revision overflow".to_owned())
            })?;
            capability.created_at = existing.created_at;
            capability.updated_at = now;
            if security_changed {
                capability.enabled = false;
                capability.approved_at = Timestamp::Zero;
                capability.expires_at = Timestamp::Zero;
            } else {
                capability.enabled = existing.enabled;
                capability.approved_at = existing.approved_at;
                capability.expires_at = existing.expires_at;
            }
            typed::put(transaction, RecordKey::Id(capability.id), &capability)?;
            Ok(capability)
        })
    }

    /// Enables only a transaction-local record which independently matches an
    /// opaque proof minted by pure normalization and explicit digest confirmation.
    pub fn approve_capability(&self, proof: ApprovalProof) -> StoreResult<Capability> {
        let now = self.clock.now_local();
        self.write(|transaction| {
            let id = proof.capability_id();
            let mut capability = required_write::<Capability>(transaction, RecordKey::Id(id))?;
            require_capability_revision(proof.revision(), capability.revision)?;
            let preview = normalize(&capability).map_err(|_| StoreError::CapabilityScopeChanged)?;
            if preview.capability != capability
                || capability.scope_digest != preview.scope_digest
                || !proof.matches(id, capability.revision, preview.scope_digest)
            {
                return Err(StoreError::CapabilityScopeChanged);
            }
            capability.enabled = true;
            capability.approved_at = now;
            capability.expires_at =
                timestamp_add_seconds(now, capability.approval_duration_seconds)?;
            capability.revision = capability.revision.checked_add(1).ok_or_else(|| {
                StoreError::InvalidManifest("capability revision overflow".to_owned())
            })?;
            capability.updated_at = now;
            typed::put(transaction, RecordKey::Id(id), &capability)?;
            Ok(capability)
        })
    }

    /// Revokes an approval under the draft revision fence.
    pub fn disable_capability(&self, id: u64, expected_revision: u64) -> StoreResult<Capability> {
        let now = self.clock.now_local();
        self.write(|transaction| {
            let mut capability = required_write::<Capability>(transaction, RecordKey::Id(id))?;
            require_capability_revision(expected_revision, capability.revision)?;
            capability.enabled = false;
            capability.approved_at = Timestamp::Zero;
            capability.expires_at = Timestamp::Zero;
            capability.revision = capability.revision.checked_add(1).ok_or_else(|| {
                StoreError::InvalidManifest("capability revision overflow".to_owned())
            })?;
            capability.updated_at = now;
            typed::put(transaction, RecordKey::Id(id), &capability)?;
            Ok(capability)
        })
    }

    /// Expires an enabled approval at the storage-owned current time.
    pub fn expire_capability(&self, id: u64, expected_revision: u64) -> StoreResult<Capability> {
        let now = self.clock.now_local();
        self.write(|transaction| {
            let mut capability = required_write::<Capability>(transaction, RecordKey::Id(id))?;
            require_capability_revision(expected_revision, capability.revision)?;
            if !capability.enabled || capability.approved_at.is_zero() {
                return Err(StoreError::CapabilityNotEnabled);
            }
            capability.expires_at = now;
            capability.revision = capability.revision.checked_add(1).ok_or_else(|| {
                StoreError::InvalidManifest("capability revision overflow".to_owned())
            })?;
            capability.updated_at = now;
            typed::put(transaction, RecordKey::Id(id), &capability)?;
            Ok(capability)
        })
    }

    pub fn delete_capability(&self, id: u64, expected_revision: u64) -> StoreResult<()> {
        self.write(|transaction| {
            let existing = required_write::<Capability>(transaction, RecordKey::Id(id))?;
            require_capability_revision(expected_revision, existing.revision)?;
            transaction.delete(Collection::Capabilities, RecordKey::Id(id))?;
            Ok(())
        })
    }

    /// Appends already-sanitized metadata and enforces both retention ceilings
    /// atomically. The hard global ceiling is not caller-configurable.
    pub fn record_capability_audit(&self, audit: SanitizedAudit) -> StoreResult<CapabilityAudit> {
        let (audit, per_capability_keep) = audit.into_store_parts(self.clock.now_local());
        if !(0..=1_000).contains(&per_capability_keep) {
            return Err(StoreError::InvalidBoundedLimit);
        }
        self.add_capability_audit_bounded(audit, per_capability_keep, CAPABILITY_AUDIT_GLOBAL_LIMIT)
    }

    pub(crate) fn add_capability_audit_bounded(
        &self,
        mut audit: CapabilityAudit,
        per_capability_keep: i64,
        total_keep: i64,
    ) -> StoreResult<CapabilityAudit> {
        let now = self.clock.now_local();
        self.write(|transaction| {
            audit.id = transaction.next_id(Collection::CapabilityAudits)?;
            if audit.created_at == Timestamp::Zero {
                audit.created_at = now;
            }
            typed::put(transaction, RecordKey::Id(audit.id), &audit)?;
            prune_audits(
                transaction,
                audit.capability_id,
                if per_capability_keep > 0 {
                    per_capability_keep
                } else {
                    -1
                },
                if total_keep > 0 { total_keep } else { -1 },
            )?;
            Ok(audit)
        })
    }

    pub fn capability_audits(
        &self,
        capability_id: u64,
        limit: usize,
    ) -> StoreResult<Vec<CapabilityAudit>> {
        let mut values = self.list::<CapabilityAudit>()?;
        values.reverse();
        if capability_id != 0 {
            values.retain(|audit| audit.capability_id == capability_id);
        }
        if limit > 0 {
            values.truncate(limit);
        }
        Ok(values)
    }

    pub fn prune_capability_audits(&self, capability_id: u64, keep: i64) -> StoreResult<()> {
        self.write(|transaction| {
            let keep = keep.max(0);
            if keep == 0 {
                for audit in typed::scan_write::<CapabilityAudit>(transaction)? {
                    if audit.capability_id == capability_id {
                        transaction
                            .delete(Collection::CapabilityAudits, RecordKey::Id(audit.id))?;
                    }
                }
                Ok(())
            } else {
                prune_audits(transaction, capability_id, keep, -1)
            }
        })
    }

    pub fn write_memory(&self, request: MemoryWriteRequest) -> StoreResult<MemoryWriteResult> {
        validate_memory_request(&request)?;
        if request.workspace_generation != self.active.binding.generation {
            return Err(StoreError::StaleWorkspaceGeneration {
                expected: request.workspace_generation,
                active: self.active.binding.generation,
            });
        }
        let digest = Digest32(crate::sha256::digest(
            memory_digest_json(&request).as_bytes(),
        ));
        let now = self.clock.now_utc();
        let writer = self.writer_version.clone();
        self.write(|transaction| {
            if let Some(record) = typed::get_write::<MemoryWritebackRecord>(
                transaction,
                RecordKey::Bytes(request.request_id.as_bytes()),
            )? {
                if record.digest != digest {
                    return Err(StoreError::MemoryWritebackReplay);
                }
                return if record.kind == MemoryKind::Summary {
                    Ok(MemoryWriteResult {
                        note: None,
                        summary: request.body,
                        replayed: true,
                    })
                } else {
                    Ok(MemoryWriteResult {
                        note: Some(required_write::<Note>(
                            transaction,
                            RecordKey::Id(record.note_id),
                        )?),
                        summary: String::new(),
                        replayed: true,
                    })
                };
            }
            validate_memory_target(transaction, &request)?;
            let sequence = transaction.next_id(Collection::MemoryWritebacks)?;
            let mut receipt = MemoryWritebackRecord {
                digest,
                sequence,
                kind: request.kind,
                note_id: 0,
            };
            let result = if request.kind == MemoryKind::Summary {
                let mut meta = required_write::<Meta>(transaction, RecordKey::Singleton)?;
                meta.summary.clone_from(&request.body);
                stamp_meta(&mut meta, now, writer);
                typed::put(transaction, RecordKey::Singleton, &meta)?;
                MemoryWriteResult {
                    note: None,
                    summary: request.body.clone(),
                    replayed: false,
                }
            } else {
                let id = transaction.next_id(Collection::Notes)?;
                let note = Note {
                    id,
                    target: request.target,
                    target_id: request.target_id,
                    kind: request.kind,
                    body: request.body.clone(),
                    created_at: now,
                };
                typed::put(transaction, RecordKey::Id(id), &note)?;
                receipt.note_id = id;
                MemoryWriteResult {
                    note: Some(note),
                    summary: String::new(),
                    replayed: false,
                }
            };
            let envelope = typed::encode(&receipt)?;
            transaction.put(
                Collection::MemoryWritebacks,
                RecordKey::Bytes(request.request_id.as_bytes()),
                &envelope,
            )?;
            prune_memory_receipts(transaction)?;
            Ok(result)
        })
    }

    pub fn snapshot(&self) -> StoreResult<ProjectSnapshot> {
        self.active.store.read(|transaction| {
            let meta =
                typed::get(transaction, RecordKey::Singleton)?.ok_or(StoreError::NotFound)?;
            Ok(ProjectSnapshot::new(
                meta,
                typed::scan(transaction)?,
                typed::scan(transaction)?,
                typed::scan(transaction)?,
                typed::scan(transaction)?,
                typed::scan(transaction)?,
                typed::scan(transaction)?,
            ))
        })
    }

    pub fn counts(&self) -> StoreResult<Counts> {
        Ok(self.snapshot()?.counts())
    }

    fn get_id<R: StoredRecord>(&self, id: u64) -> StoreResult<R> {
        self.active.store.read(|transaction| {
            typed::get(transaction, RecordKey::Id(id))?.ok_or(StoreError::NotFound)
        })
    }

    fn list<R: StoredRecord>(&self) -> StoreResult<Vec<R>> {
        self.active.store.read(typed::scan::<R>)
    }

    fn list_ordered<R: StoredRecord + Ordered>(&self) -> StoreResult<Vec<R>> {
        let mut values = self.list::<R>()?;
        values.sort_by_key(|value| (value.order(), value.id()));
        Ok(values)
    }

    fn mutate_id<R: StoredRecord>(
        &self,
        id: u64,
        mutate: impl FnOnce(&mut R, Timestamp),
    ) -> StoreResult<()> {
        let now = self.clock.now_local();
        self.write(|transaction| {
            let mut value = required_write::<R>(transaction, RecordKey::Id(id))?;
            mutate(&mut value, now);
            typed::put(transaction, RecordKey::Id(id), &value)?;
            Ok(())
        })
    }

    fn migrate_legacy_meta(&self) -> StoreResult<()> {
        let meta = self.meta()?;
        if meta.format_version == CURRENT_PROJECT_FORMAT {
            return Ok(());
        }
        let now = self.clock.now_local();
        let writer = self.writer_version.clone();
        self.write(|transaction| {
            let mut current = required_write::<Meta>(transaction, RecordKey::Singleton)?;
            if current.format_version >= CURRENT_PROJECT_FORMAT {
                return Ok(());
            }
            current.format_version = CURRENT_PROJECT_FORMAT;
            stamp_meta(&mut current, now, writer);
            typed::put(transaction, RecordKey::Singleton, &current)?;
            Ok(())
        })
    }
}

trait Ordered {
    fn id(&self) -> u64;
    fn order(&self) -> i64;
}

macro_rules! ordered {
    ($type:ty) => {
        impl Ordered for $type {
            fn id(&self) -> u64 {
                self.id
            }
            fn order(&self) -> i64 {
                self.order
            }
        }
    };
}
ordered!(Milestone);
ordered!(Plan);
ordered!(Task);

fn required_write<R: StoredRecord>(
    transaction: &WriteTransaction,
    key: RecordKey<'_>,
) -> StoreResult<R> {
    typed::get_write(transaction, key)?.ok_or(StoreError::NotFound)
}

fn require_id_write<R: StoredRecord>(transaction: &WriteTransaction, id: u64) -> StoreResult<()> {
    required_write::<R>(transaction, RecordKey::Id(id)).map(|_| ())
}

fn count_write<R: StoredRecord>(transaction: &WriteTransaction) -> StoreResult<i64> {
    i64::try_from(typed::scan_write::<R>(transaction)?.len())
        .map_err(|_| StoreError::InvalidManifest("record count exceeds i64".to_owned()))
}

fn stamp_meta(meta: &mut Meta, now: Timestamp, writer_version: String) {
    meta.updated_at = now;
    meta.last_write_version = writer_version;
}

fn same_instant(left: Timestamp, right: Timestamp) -> bool {
    match (left.unix_nanoseconds(), right.unix_nanoseconds()) {
        (Some(left), Some(right)) => left == right,
        (None, None) => true,
        _ => false,
    }
}

fn capability_security_changed(left: &Capability, right: &Capability) -> bool {
    left.kind != right.kind
        || left.agent_profile != right.agent_profile
        || left.approval_duration_seconds != right.approval_duration_seconds
        || left.limits != right.limits
        || left.audit != right.audit
        || left.http != right.http
        || left.git != right.git
        || left.ssh != right.ssh
        || left.scope_digest != right.scope_digest
}

fn require_capability_revision(expected: u64, actual: u64) -> StoreResult<()> {
    if expected == actual {
        Ok(())
    } else {
        Err(StoreError::CapabilityRevisionChanged { expected, actual })
    }
}

fn timestamp_add_seconds(value: Timestamp, seconds: i64) -> StoreResult<Timestamp> {
    let Timestamp::Fixed {
        seconds: base,
        nanoseconds,
        offset_seconds,
    } = value
    else {
        return Err(StoreError::InvalidManifest(
            "capability approval time must be set".to_owned(),
        ));
    };
    Ok(Timestamp::Fixed {
        seconds: base.checked_add(seconds).ok_or_else(|| {
            StoreError::InvalidManifest("capability approval expiry overflow".to_owned())
        })?,
        nanoseconds,
        offset_seconds,
    })
}

fn prune_audits(
    transaction: &mut WriteTransaction,
    capability_id: u64,
    per_capability_keep: i64,
    total_keep: i64,
) -> StoreResult<()> {
    let mut audits = typed::scan_write::<CapabilityAudit>(transaction)?;
    audits.sort_by_key(|audit| std::cmp::Reverse(audit.id));
    let mut matching = 0_i64;
    for (index, audit) in audits.into_iter().enumerate() {
        let total = i64::try_from(index + 1).unwrap_or(i64::MAX);
        let mut remove = total_keep > 0 && total > total_keep;
        if audit.capability_id == capability_id {
            matching += 1;
            remove |= per_capability_keep > 0 && matching > per_capability_keep;
        }
        if remove {
            transaction.delete(Collection::CapabilityAudits, RecordKey::Id(audit.id))?;
        }
    }
    Ok(())
}

fn validate_memory_request(request: &MemoryWriteRequest) -> StoreResult<()> {
    if request.request_id.is_empty()
        || request.request_id.len() > 128
        || !request
            .request_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return Err(StoreError::InvalidMemoryWriteback(
            "invalid request ID".to_owned(),
        ));
    }
    if request.body.is_empty() {
        return Err(StoreError::InvalidMemoryWriteback(
            "content is required".to_owned(),
        ));
    }
    if request.workspace_generation == 0
        || request.session_id.is_empty()
        || request.session_id.len() > 128
        || request.association_revision == 0
    {
        return Err(StoreError::InvalidMemoryWriteback(
            "source association is required".to_owned(),
        ));
    }
    if !matches!(
        request.kind,
        MemoryKind::Summary | MemoryKind::Decision | MemoryKind::Blocker | MemoryKind::Handoff
    ) {
        return Err(StoreError::InvalidMemoryWriteback(format!(
            "unsupported kind {:?}",
            request.kind
        )));
    }
    Ok(())
}

fn validate_memory_target(
    transaction: &WriteTransaction,
    request: &MemoryWriteRequest,
) -> StoreResult<()> {
    match request.target {
        NoteTarget::Project if request.target_id == 0 && request.plan_id == 0 => Ok(()),
        NoteTarget::Project => Err(StoreError::InvalidMemoryWriteback(
            "invalid project target".to_owned(),
        )),
        NoteTarget::Plan => {
            if request.target_id == 0
                || request.plan_id != request.target_id
                || typed::get_write::<Plan>(transaction, RecordKey::Id(request.target_id))?
                    .is_none()
            {
                Err(StoreError::InvalidMemoryWriteback(
                    "plan target no longer exists".to_owned(),
                ))
            } else {
                Ok(())
            }
        }
        NoteTarget::Task => {
            if request.target_id == 0 || request.plan_id == 0 {
                return Err(StoreError::InvalidMemoryWriteback(
                    "invalid task target".to_owned(),
                ));
            }
            let task = typed::get_write::<Task>(transaction, RecordKey::Id(request.target_id))?
                .ok_or_else(|| {
                    StoreError::InvalidMemoryWriteback("task target no longer exists".to_owned())
                })?;
            if task.plan_id != request.plan_id
                || typed::get_write::<Plan>(transaction, RecordKey::Id(request.plan_id))?.is_none()
            {
                Err(StoreError::InvalidMemoryWriteback(
                    "task target changed plans".to_owned(),
                ))
            } else {
                Ok(())
            }
        }
    }
}

fn prune_memory_receipts(transaction: &mut WriteTransaction) -> StoreResult<()> {
    let mut receipts = BTreeMap::new();
    for (key, envelope) in transaction.scan(Collection::MemoryWritebacks)? {
        let crate::OwnedRecordKey::Bytes(key) = key else {
            return Err(StoreError::InvalidManifest(
                "memory receipt key is not bytes".to_owned(),
            ));
        };
        let receipt = typed::decode::<MemoryWritebackRecord>(envelope)?;
        receipts.insert(receipt.sequence, key);
    }
    while receipts.len() > MEMORY_WRITEBACK_REPLAY_LIMIT {
        let (&sequence, key) = receipts.first_key_value().expect("nonempty receipt map");
        transaction.delete(Collection::MemoryWritebacks, RecordKey::Bytes(key))?;
        receipts.remove(&sequence);
    }
    Ok(())
}

fn memory_digest_json(request: &MemoryWriteRequest) -> String {
    format!(
        "{{\"kind\":{},\"body\":{},\"target\":{},\"target_id\":{},\"plan_id\":{},\"generation\":{},\"session_id\":{},\"revision\":{}}}",
        json_string(request.kind.as_str()),
        json_string(&request.body),
        json_string(request.target.as_str()),
        request.target_id,
        request.plan_id,
        request.workspace_generation,
        json_string(&request.session_id),
        request.association_revision,
    )
}

fn json_string(value: &str) -> String {
    let mut result = String::with_capacity(value.len() + 2);
    result.push('"');
    for character in value.chars() {
        match character {
            '"' => result.push_str("\\\""),
            '\\' => result.push_str("\\\\"),
            '\u{08}' => result.push_str("\\b"),
            '\u{0c}' => result.push_str("\\f"),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            '<' => result.push_str("\\u003c"),
            '>' => result.push_str("\\u003e"),
            '&' => result.push_str("\\u0026"),
            '\u{2028}' => result.push_str("\\u2028"),
            '\u{2029}' => result.push_str("\\u2029"),
            value if value <= '\u{1f}' => {
                use std::fmt::Write;
                write!(result, "\\u{:04x}", u32::from(value)).expect("write to String");
            }
            value => result.push(value),
        }
    }
    result.push('"');
    result
}

fn format_timestamp_utc(value: Timestamp) -> String {
    let Timestamp::Fixed {
        seconds,
        nanoseconds,
        ..
    } = value
    else {
        return "0001-01-01T00:00:00Z".to_owned();
    };
    let days = i128::from(seconds).div_euclid(86_400);
    let seconds_of_day = i128::from(seconds).rem_euclid(86_400);
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    let hour = seconds_of_day / 3_600;
    let minute = seconds_of_day % 3_600 / 60;
    let second = seconds_of_day % 60;
    let mut output = format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}");
    if nanoseconds != 0 {
        let fraction = format!("{nanoseconds:09}");
        output.push('.');
        output.push_str(fraction.trim_end_matches('0'));
    }
    output.push('Z');
    output
}
