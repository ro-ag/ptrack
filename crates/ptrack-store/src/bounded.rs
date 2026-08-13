use std::collections::{BTreeMap, BTreeSet};

use ptrack_core::{Commit, Issue, IssueStatus, Note, NoteTarget, Plan, Task, TaskStatus};

use crate::typed;
use crate::{Collection, ProjectStore, StoreError, StoreResult};

pub const MAX_BOUNDED_READ: usize = 1_000;
pub const MAX_ASSOCIATION_SCAN: usize = 10_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Bounded<T> {
    pub items: Vec<T>,
    pub total: usize,
    pub more: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScanBounded<T> {
    pub items: Vec<T>,
    pub scanned: usize,
    pub scan_limit: usize,
    pub truncated: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TaskProgress {
    pub total: usize,
    pub done: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TaskAssociations {
    pub note_counts: BTreeMap<u64, usize>,
    pub commit_counts: BTreeMap<u64, usize>,
    pub issue_counts: BTreeMap<u64, usize>,
    pub latest_notes: BTreeMap<u64, String>,
}

impl ProjectStore {
    pub fn plans_bounded(&self, limit: usize) -> StoreResult<Bounded<Plan>> {
        check(limit)?;
        self.read(|transaction| {
            let total = transaction.collection_len(Collection::Plans)?;
            Ok(bound(
                typed::scan_limited(transaction, limit, false)?,
                total,
            ))
        })
    }
    pub fn tasks_by_plan_bounded(&self, plan_id: u64, limit: usize) -> StoreResult<Bounded<Task>> {
        check(limit)?;
        self.filtered_tasks(limit, |task| task.plan_id == plan_id)
    }
    pub fn blocked_tasks_bounded(&self, limit: usize) -> StoreResult<Bounded<Task>> {
        check(limit)?;
        self.filtered_tasks(limit, |task| task.status == TaskStatus::Blocked)
    }
    pub fn recent_notes_bounded(&self, limit: usize) -> StoreResult<Bounded<Note>> {
        check(limit)?;
        self.read(|transaction| {
            let total = transaction.collection_len(Collection::Notes)?;
            Ok(bound(typed::scan_limited(transaction, limit, true)?, total))
        })
    }
    pub fn recent_commits_bounded(&self, limit: usize) -> StoreResult<Bounded<Commit>> {
        check(limit)?;
        self.read(|transaction| {
            let total = transaction.collection_len(Collection::Commits)?;
            Ok(bound(typed::scan_limited(transaction, limit, true)?, total))
        })
    }
    pub fn open_issues_bounded(&self, limit: usize) -> StoreResult<Bounded<Issue>> {
        check(limit)?;
        self.read(|transaction| {
            let mut items = Vec::with_capacity(limit);
            let mut total = 0;
            typed::visit::<Issue>(transaction, true, |issue| {
                if issue.status == IssueStatus::Open {
                    total += 1;
                    if items.len() < limit {
                        items.push(issue);
                    }
                }
                Ok(())
            })?;
            Ok(bound(items, total))
        })
    }
    pub fn open_issues_scan_bounded(&self, limit: usize) -> StoreResult<ScanBounded<Issue>> {
        check(limit)?;
        let total = self.read(|transaction| transaction.collection_len(Collection::Issues))?;
        let mut values =
            self.read(|transaction| typed::scan_limited::<Issue>(transaction, limit, true))?;
        let scanned = values.len();
        values.retain(|v| v.status == IssueStatus::Open);
        Ok(ScanBounded {
            items: values,
            scanned,
            scan_limit: limit,
            truncated: total > scanned,
        })
    }
    pub fn plan_task_progress(&self, plan_id: u64) -> StoreResult<TaskProgress> {
        self.read(|transaction| {
            let mut result = TaskProgress::default();
            typed::visit::<Task>(transaction, false, |task| {
                if task.plan_id == plan_id {
                    result.total += 1;
                    if task.status == TaskStatus::Done {
                        result.done += 1;
                    }
                }
                Ok(())
            })?;
            Ok(result)
        })
    }
    pub fn task_associations(&self, ids: &BTreeSet<u64>) -> StoreResult<TaskAssociations> {
        if ids.len() > MAX_BOUNDED_READ {
            return Err(StoreError::InvalidBoundedLimit);
        }
        self.read(|tx| {
            for collection in [Collection::Notes, Collection::Commits, Collection::Issues] {
                if tx.collection_len(collection)? > MAX_ASSOCIATION_SCAN {
                    return Err(StoreError::BoundedScanLimit {
                        collection: collection.name(),
                        maximum: MAX_ASSOCIATION_SCAN,
                    });
                }
            }
            let mut out = TaskAssociations::default();
            typed::visit::<Note>(tx, true, |v| {
                if v.target == NoteTarget::Task && ids.contains(&v.target_id) {
                    *out.note_counts.entry(v.target_id).or_default() += 1;
                    out.latest_notes.entry(v.target_id).or_insert(v.body);
                }
                Ok(())
            })?;
            typed::visit::<Commit>(tx, false, |v| {
                if ids.contains(&v.task_id) {
                    *out.commit_counts.entry(v.task_id).or_default() += 1;
                }
                Ok(())
            })?;
            typed::visit::<Issue>(tx, false, |v| {
                if v.status == IssueStatus::Open && ids.contains(&v.task_id) {
                    *out.issue_counts.entry(v.task_id).or_default() += 1;
                }
                Ok(())
            })?;
            Ok(out)
        })
    }

    fn filtered_tasks(
        &self,
        limit: usize,
        keep: impl Fn(Task) -> bool,
    ) -> StoreResult<Bounded<Task>> {
        self.read(|transaction| {
            let mut items = Vec::with_capacity(limit);
            let mut total = 0;
            typed::visit::<Task>(transaction, false, |task| {
                if keep(task.clone()) {
                    total += 1;
                    if items.len() < limit {
                        items.push(task);
                    }
                }
                Ok(())
            })?;
            Ok(bound(items, total))
        })
    }
}

fn check(limit: usize) -> StoreResult<()> {
    if (1..=MAX_BOUNDED_READ).contains(&limit) {
        Ok(())
    } else {
        Err(StoreError::InvalidBoundedLimit)
    }
}
fn bound<T>(items: Vec<T>, total: usize) -> Bounded<T> {
    Bounded {
        more: total.saturating_sub(items.len()),
        items,
        total,
    }
}
