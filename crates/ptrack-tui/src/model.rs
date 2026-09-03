use std::path::PathBuf;

use ptrack_app::{AgentHandoffInbox, AgentRunObservationV1, AgentRunsV2, Mutation};
use ptrack_core::{Issue, Milestone, Plan, ProjectSnapshot, Task, TaskStatus};

use crate::InputEditor;

pub const TAB_NAMES: [&str; 6] = [
    "Overview",
    "Board",
    "Milestones",
    "Issues",
    "Maintenance",
    "Agents",
];
pub const BOARD_STATUSES: [TaskStatus; 4] = [
    TaskStatus::Todo,
    TaskStatus::Doing,
    TaskStatus::Blocked,
    TaskStatus::Done,
];
pub const BOARD_TITLES: [&str; 4] = ["Todo", "Doing", "Blocked", "Done"];

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Tab {
    #[default]
    Overview,
    Board,
    Milestones,
    Issues,
    Maintenance,
    Agents,
}

impl Tab {
    pub(crate) const fn index(self) -> usize {
        match self {
            Self::Overview => 0,
            Self::Board => 1,
            Self::Milestones => 2,
            Self::Issues => 3,
            Self::Maintenance => 4,
            Self::Agents => 5,
        }
    }

    pub(crate) const fn from_index(value: usize) -> Self {
        match value % 6 {
            0 => Self::Overview,
            1 => Self::Board,
            2 => Self::Milestones,
            3 => Self::Issues,
            4 => Self::Maintenance,
            _ => Self::Agents,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum AgentPane {
    #[default]
    Runs,
    Handoffs,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeContext {
    pub project_root: PathBuf,
    pub database: PathBuf,
    pub global_home: PathBuf,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum PaneFocus {
    #[default]
    Plans,
    Tasks,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InputPurpose {
    AddPlan,
    AddTask,
    AddMilestone,
    AddIssue,
    AddNote,
    EditGoal,
    EditSummary,
    Rename,
    MoveTask,
    ConvertTask,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ActiveInput {
    pub purpose: InputPurpose,
    pub prompt: String,
    pub editor: InputEditor,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DetailTarget {
    Plan(u64),
    Task(u64),
    Milestone(u64),
    Issue(u64),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Success {
    Message(String),
    MovedCard { message: String, column: usize },
    Added(&'static str),
    ConvertedTask(u64),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Effect {
    Quit,
    Reload {
        success: String,
        reopen_detail: bool,
    },
    Backup,
    LoadAgentDetail(String),
    Mutate {
        mutation: Mutation,
        success: Success,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Model {
    pub snapshot: ProjectSnapshot,
    pub context: RuntimeContext,
    pub tab: Tab,
    pub(crate) focus: PaneFocus,
    pub(crate) plan_cursor: usize,
    pub(crate) task_cursor: usize,
    pub(crate) board_col: usize,
    pub(crate) board_row: usize,
    pub(crate) milestone_cursor: usize,
    pub(crate) issue_cursor: usize,
    pub(crate) agent_runs: Option<AgentRunsV2>,
    pub(crate) agent_inbox: Option<AgentHandoffInbox>,
    pub(crate) agent_error: String,
    pub(crate) agent_pane: AgentPane,
    pub(crate) agent_cursor: usize,
    pub(crate) handoff_cursor: usize,
    pub(crate) agent_detail: Option<AgentRunObservationV1>,
    pub(crate) agent_detail_offset: usize,
    pub(crate) input: Option<ActiveInput>,
    pub(crate) pending_task_id: u64,
    pub(crate) detail: Option<DetailTarget>,
    pub(crate) detail_offset: usize,
    pub(crate) welcome: bool,
    pub(crate) menu: bool,
    pub(crate) menu_cursor: usize,
    pub status: String,
    pub(crate) width: u16,
    pub(crate) height: u16,
}

impl Model {
    #[must_use]
    pub fn new(snapshot: ProjectSnapshot, context: RuntimeContext) -> Self {
        let plan_cursor = snapshot
            .plans
            .iter()
            .position(|plan| plan.id == snapshot.meta.active_plan)
            .unwrap_or_default();
        Self {
            snapshot,
            context,
            tab: Tab::Overview,
            focus: PaneFocus::Plans,
            plan_cursor,
            task_cursor: 0,
            board_col: 0,
            board_row: 0,
            milestone_cursor: 0,
            issue_cursor: 0,
            agent_runs: None,
            agent_inbox: None,
            agent_error: String::new(),
            agent_pane: AgentPane::Runs,
            agent_cursor: 0,
            handoff_cursor: 0,
            agent_detail: None,
            agent_detail_offset: 0,
            input: None,
            pending_task_id: 0,
            detail: None,
            detail_offset: 0,
            welcome: true,
            menu: false,
            menu_cursor: 0,
            status: String::new(),
            width: 100,
            height: 30,
        }
    }

    pub fn resize(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
    }

    pub fn replace_snapshot(&mut self, snapshot: ProjectSnapshot) {
        self.snapshot = snapshot;
        self.clamp_cursors();
    }

    pub(crate) fn replace_agent_state(
        &mut self,
        runs: Option<AgentRunsV2>,
        inbox: Option<AgentHandoffInbox>,
        error: String,
    ) {
        let selected_run = self
            .agent_runs
            .as_ref()
            .and_then(|value| value.runs.get(self.agent_cursor))
            .map(|value| value.run_id.clone());
        let selected_handoff = self
            .agent_inbox
            .as_ref()
            .and_then(|value| value.items.get(self.handoff_cursor))
            .map(|value| value.id.clone());
        self.agent_runs = runs;
        self.agent_inbox = inbox;
        self.agent_error = error;
        self.agent_cursor = selected_run
            .as_deref()
            .and_then(|id| {
                self.agent_runs
                    .as_ref()?
                    .runs
                    .iter()
                    .position(|run| run.run_id == id)
            })
            .unwrap_or_else(|| {
                clamp_index(
                    self.agent_cursor,
                    self.agent_runs.as_ref().map_or(0, |value| value.runs.len()),
                )
            });
        self.handoff_cursor = selected_handoff
            .as_deref()
            .and_then(|id| {
                self.agent_inbox
                    .as_ref()?
                    .items
                    .iter()
                    .position(|handoff| handoff.id == id)
            })
            .unwrap_or_else(|| {
                clamp_index(
                    self.handoff_cursor,
                    self.agent_inbox
                        .as_ref()
                        .map_or(0, |value| value.items.len()),
                )
            });
        if let Some(detail) = &self.agent_detail
            && !self
                .agent_runs
                .as_ref()
                .is_some_and(|runs| runs.runs.iter().any(|run| run.run_id == detail.run.run_id))
        {
            self.agent_detail = None;
            self.agent_detail_offset = 0;
        }
    }

    pub(crate) fn selected_agent_run_id(&self) -> Option<&str> {
        self.agent_runs
            .as_ref()?
            .runs
            .get(self.agent_cursor)
            .map(|run| run.run_id.as_str())
    }

    pub(crate) fn clamp_cursors(&mut self) {
        self.plan_cursor = clamp_index(self.plan_cursor, self.snapshot.plans.len());
        self.task_cursor = clamp_index(self.task_cursor, self.current_tasks().count());
        self.milestone_cursor = clamp_index(self.milestone_cursor, self.snapshot.milestones.len());
        self.issue_cursor = clamp_index(self.issue_cursor, self.snapshot.issues.len());
        self.agent_cursor = clamp_index(
            self.agent_cursor,
            self.agent_runs.as_ref().map_or(0, |value| value.runs.len()),
        );
        self.handoff_cursor = clamp_index(
            self.handoff_cursor,
            self.agent_inbox
                .as_ref()
                .map_or(0, |value| value.items.len()),
        );
        self.board_col = self.board_col.min(BOARD_STATUSES.len() - 1);
        self.board_row = clamp_index(self.board_row, self.board_tasks(self.board_col).count());
    }

    pub(crate) fn current_plan(&self) -> Option<&Plan> {
        self.snapshot.plans.get(self.plan_cursor)
    }

    pub(crate) fn current_tasks(&self) -> impl Iterator<Item = &Task> {
        let plan_id = self.current_plan().map_or(0, |plan| plan.id);
        self.snapshot.tasks_for_plan(plan_id)
    }

    pub(crate) fn current_task(&self) -> Option<&Task> {
        self.current_tasks().nth(self.task_cursor)
    }

    pub(crate) fn current_milestone(&self) -> Option<&Milestone> {
        self.snapshot.milestones.get(self.milestone_cursor)
    }

    pub(crate) fn current_issue(&self) -> Option<&Issue> {
        self.snapshot.issues.get(self.issue_cursor)
    }

    pub(crate) fn board_tasks(&self, column: usize) -> impl Iterator<Item = &Task> {
        let status = BOARD_STATUSES[column.min(BOARD_STATUSES.len() - 1)];
        self.current_tasks()
            .filter(move |task| task.status == status)
    }

    pub(crate) fn board_task(&self) -> Option<&Task> {
        self.board_tasks(self.board_col).nth(self.board_row)
    }

    pub(crate) fn selected_task(&self) -> Option<&Task> {
        match self.tab {
            Tab::Board => self.board_task(),
            Tab::Overview if self.focus == PaneFocus::Tasks => self.current_task(),
            _ => None,
        }
    }

    pub(crate) fn selected_detail(&self) -> Option<DetailTarget> {
        match self.tab {
            Tab::Issues => self
                .current_issue()
                .map(|value| DetailTarget::Issue(value.id)),
            Tab::Milestones => self
                .current_milestone()
                .map(|value| DetailTarget::Milestone(value.id)),
            Tab::Board => self.board_task().map(|value| DetailTarget::Task(value.id)),
            Tab::Overview if self.focus == PaneFocus::Tasks => self
                .current_task()
                .map(|value| DetailTarget::Task(value.id))
                .or_else(|| {
                    self.current_plan()
                        .map(|value| DetailTarget::Plan(value.id))
                }),
            Tab::Overview => self
                .current_plan()
                .map(|value| DetailTarget::Plan(value.id)),
            Tab::Maintenance | Tab::Agents => None,
        }
    }

    pub(crate) fn rename_target(&self) -> Option<(&'static str, u64, &str)> {
        match self.selected_detail()? {
            DetailTarget::Plan(id) => self
                .snapshot
                .plan(id)
                .map(|value| ("plan", id, value.title.as_str())),
            DetailTarget::Task(id) => self
                .snapshot
                .task(id)
                .map(|value| ("task", id, value.title.as_str())),
            DetailTarget::Milestone(id) => self
                .snapshot
                .milestone(id)
                .map(|value| ("milestone", id, value.title.as_str())),
            DetailTarget::Issue(id) => self
                .snapshot
                .issue(id)
                .map(|value| ("issue", id, value.title.as_str())),
        }
    }

    pub(crate) fn start_input(
        &mut self,
        purpose: InputPurpose,
        prompt: impl Into<String>,
        initial: &str,
    ) {
        self.input = Some(ActiveInput {
            purpose,
            prompt: prompt.into(),
            editor: InputEditor::new(initial),
        });
        self.status.clear();
    }
}

pub(crate) fn clamp_index(value: usize, length: usize) -> usize {
    if length == 0 {
        0
    } else {
        value.min(length - 1)
    }
}
