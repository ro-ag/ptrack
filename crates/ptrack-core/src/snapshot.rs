use crate::{
    Commit, Counts, Issue, IssueStatus, Meta, Milestone, MilestoneStatus, Note, NoteTarget, Plan,
    PlanStatus, Task, TaskStatus,
};

/// One dependency-free, consistent project read used by shared query and
/// reporting services.
///
/// Capability records are intentionally absent: imported grants remain inert
/// until the separate capability-policy layer validates and activates them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectSnapshot {
    pub meta: Meta,
    pub milestones: Vec<Milestone>,
    pub plans: Vec<Plan>,
    pub tasks: Vec<Task>,
    pub issues: Vec<Issue>,
    pub notes: Vec<Note>,
    pub commits: Vec<Commit>,
}

impl ProjectSnapshot {
    /// Builds a snapshot and normalizes every externally visible list order.
    #[must_use]
    pub fn new(
        meta: Meta,
        mut milestones: Vec<Milestone>,
        mut plans: Vec<Plan>,
        mut tasks: Vec<Task>,
        mut issues: Vec<Issue>,
        mut notes: Vec<Note>,
        mut commits: Vec<Commit>,
    ) -> Self {
        milestones.sort_by_key(|milestone| (milestone.order, milestone.id));
        plans.sort_by_key(|plan| (plan.order, plan.id));
        tasks.sort_by_key(|task| (task.order, task.id));
        issues.sort_by_key(|issue| issue.id);
        notes.sort_by_key(|note| note.id);
        commits.sort_by_key(|commit| commit.id);
        Self {
            meta,
            milestones,
            plans,
            tasks,
            issues,
            notes,
            commits,
        }
    }

    /// Finds a milestone by its persistent ID.
    #[must_use]
    pub fn milestone(&self, id: u64) -> Option<&Milestone> {
        self.milestones.iter().find(|milestone| milestone.id == id)
    }

    /// Finds a plan by its persistent ID.
    #[must_use]
    pub fn plan(&self, id: u64) -> Option<&Plan> {
        self.plans.iter().find(|plan| plan.id == id)
    }

    /// Finds a task by its persistent ID.
    #[must_use]
    pub fn task(&self, id: u64) -> Option<&Task> {
        self.tasks.iter().find(|task| task.id == id)
    }

    /// Finds an issue by its persistent ID.
    #[must_use]
    pub fn issue(&self, id: u64) -> Option<&Issue> {
        self.issues.iter().find(|issue| issue.id == id)
    }

    /// Iterates plans belonging to one milestone in normalized display order.
    pub fn plans_for_milestone(&self, milestone_id: u64) -> impl Iterator<Item = &Plan> {
        self.plans
            .iter()
            .filter(move |plan| plan.milestone_id == milestone_id)
    }

    /// Iterates tasks belonging to one plan in normalized display order.
    pub fn tasks_for_plan(&self, plan_id: u64) -> impl Iterator<Item = &Task> {
        self.tasks
            .iter()
            .filter(move |task| task.plan_id == plan_id)
    }

    /// Iterates notes attached directly to a plan in insertion order.
    pub fn notes_for_plan(&self, plan_id: u64) -> impl Iterator<Item = &Note> {
        self.notes
            .iter()
            .filter(move |note| note.target == NoteTarget::Plan && note.target_id == plan_id)
    }

    /// Iterates notes attached directly to a task in insertion order.
    pub fn notes_for_task(&self, task_id: u64) -> impl Iterator<Item = &Note> {
        self.notes
            .iter()
            .filter(move |note| note.target == NoteTarget::Task && note.target_id == task_id)
    }

    /// Returns the newest notes first. A zero limit preserves the Go service's
    /// convention of returning all notes.
    #[must_use]
    pub fn recent_notes(&self, limit: usize) -> Vec<&Note> {
        let take = if limit == 0 {
            self.notes.len()
        } else {
            limit.min(self.notes.len())
        };
        self.notes.iter().rev().take(take).collect()
    }

    /// Computes the project-wide bounded-report inventory.
    #[must_use]
    pub fn counts(&self) -> Counts {
        Counts {
            milestones: self.milestones.len(),
            milestones_done: self
                .milestones
                .iter()
                .filter(|milestone| milestone.status == MilestoneStatus::Done)
                .count(),
            plans: self.plans.len(),
            plans_done: self
                .plans
                .iter()
                .filter(|plan| plan.status == PlanStatus::Done)
                .count(),
            plans_on_hold: self
                .plans
                .iter()
                .filter(|plan| plan.hold_reason.is_some())
                .count(),
            tasks: self.tasks.len(),
            tasks_done: self
                .tasks
                .iter()
                .filter(|task| task.status == TaskStatus::Done)
                .count(),
            tasks_blocked: self
                .tasks
                .iter()
                .filter(|task| task.status == TaskStatus::Blocked)
                .count(),
            tasks_open: self
                .tasks
                .iter()
                .filter(|task| task.status.is_open())
                .count(),
            tasks_on_hold: self
                .tasks
                .iter()
                .filter(|task| task.hold_reason.is_some())
                .count(),
            issues: self.issues.len(),
            issues_open: self
                .issues
                .iter()
                .filter(|issue| issue.status == IssueStatus::Open)
                .count(),
            commits: self.commits.len(),
            notes: self.notes.len(),
        }
    }
}
