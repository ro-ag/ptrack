use ptrack_core::{
    Commit, Issue, IssueStatus, MemoryKind, Meta, Milestone, MilestoneStatus, Note, NoteTarget,
    Plan, PlanStatus, ProjectSnapshot, Severity, Task, TaskStatus, Timestamp,
};
use serde_json::{Value, json};
use time::{OffsetDateTime, UtcOffset};

use crate::insights::insights_at;

const DAY: i64 = 24 * 60 * 60;

fn at(seconds: i64) -> Timestamp {
    Timestamp::Fixed {
        seconds,
        nanoseconds: 0,
        offset_seconds: 0,
    }
}

fn meta() -> Meta {
    Meta {
        goal: String::new(),
        summary: String::new(),
        active_plan: 0,
        created_at: Timestamp::Zero,
        updated_at: Timestamp::Zero,
        format_version: 1,
        last_write_version: String::new(),
        active_plans: Vec::new(),
        actors: Vec::new(),
        stack: None,
    }
}

fn plan(id: u64, title: &str, status: PlanStatus) -> Plan {
    Plan {
        id,
        title: title.to_owned(),
        status,
        milestone_id: 0,
        order: i64::try_from(id).unwrap_or_default(),
        created_at: at(0),
        updated_at: at(0),
        hold_reason: None,
        actor: None,
        ulid: None,
        claim_owner: None,
        claim_epoch: 0,
        claim_conflict: false,
        deps: Vec::new(),
    }
}

fn task(id: u64, plan_id: u64, status: TaskStatus, created: i64, updated: i64) -> Task {
    Task {
        id,
        plan_id,
        title: format!("task {id}"),
        status,
        order: i64::try_from(id).unwrap_or_default(),
        created_at: at(created),
        updated_at: at(updated),
        hold_reason: None,
        actor: None,
        ulid: None,
        deps: Vec::new(),
    }
}

fn note(id: u64, created: i64) -> Note {
    Note {
        id,
        target: NoteTarget::Project,
        target_id: 0,
        kind: MemoryKind::Legacy,
        body: format!("note {id}"),
        created_at: at(created),
        actor: None,
        ulid: None,
    }
}

fn commit(id: u64, created: i64) -> Commit {
    Commit {
        id,
        sha: format!("sha{id}"),
        subject: format!("commit {id}"),
        plan_id: 0,
        task_id: 0,
        created_at: at(created),
        actor: None,
        ulid: None,
    }
}

fn issue(id: u64, severity: Severity, status: IssueStatus, created: i64) -> Issue {
    Issue {
        id,
        title: format!("issue {id}"),
        body: String::new(),
        status,
        severity,
        task_id: 0,
        created_at: at(created),
        updated_at: at(created),
        actor: None,
        ulid: None,
    }
}

fn snapshot(
    plans: Vec<Plan>,
    tasks: Vec<Task>,
    issues: Vec<Issue>,
    notes: Vec<Note>,
    commits: Vec<Commit>,
) -> ProjectSnapshot {
    ProjectSnapshot::new(meta(), Vec::new(), plans, tasks, issues, notes, commits)
}

/// Every window is built against the reader's calendar day, so the aggregation
/// is pinned at a fixed instant with a fixed offset rather than "now".
fn build(snapshot: &ProjectSnapshot, weeks: i64, now_seconds: i64) -> Value {
    let now = OffsetDateTime::from_unix_timestamp(now_seconds).expect("valid instant");
    insights_at(snapshot, weeks, now, |_| UtcOffset::UTC)
}

#[test]
fn the_window_is_bounded_and_defaults_like_the_activity_heatmap() {
    let empty = snapshot(Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());

    // Zero and negative ask for the default; anything past the cap is clamped.
    for (requested, expected) in [(0, 16), (-5, 16), (1, 1), (52, 52), (500, 52)] {
        let value = build(&empty, requested, 30 * DAY);
        assert_eq!(value["weeks"], json!(expected), "weeks for {requested}");
        assert_eq!(
            value["daily"].as_array().expect("daily rows").len(),
            usize::try_from(expected * 7).expect("row count"),
            "daily rows for {requested}",
        );
    }
}

#[test]
fn an_empty_project_reports_zeroes_rather_than_missing_sections() {
    let value = build(
        &snapshot(Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new()),
        1,
        30 * DAY,
    );

    assert_eq!(value["daily"].as_array().expect("daily rows").len(), 7);
    assert!(
        value["daily"]
            .as_array()
            .expect("daily rows")
            .iter()
            .all(|day| day["notes"] == json!(0) && day["commits"] == json!(0)),
    );
    assert_eq!(value["leadTime"]["counted"], json!(0));
    assert_eq!(value["leadTime"]["medianDays"], Value::Null);
    assert_eq!(value["plans"], json!([]));
    assert_eq!(value["punchcard"], json!([]));
    assert_eq!(value["totals"]["tasks"], json!(0));
}

#[test]
fn each_kind_of_activity_is_counted_apart_on_its_local_day() {
    // 22:30 UTC on day 1; a reader two hours ahead sees it on day 2.
    let evening = DAY + 22 * 60 * 60 + 30 * 60;
    let snapshot = snapshot(
        Vec::new(),
        vec![
            task(1, 0, TaskStatus::Done, 0, evening),
            task(2, 0, TaskStatus::Todo, evening, evening),
        ],
        Vec::new(),
        vec![note(1, evening)],
        vec![commit(1, evening)],
    );
    let now = OffsetDateTime::from_unix_timestamp(3 * DAY).expect("valid instant");
    let value = insights_at(&snapshot, 1, now, |_| {
        UtcOffset::from_hms(2, 0, 0).expect("valid offset")
    });

    let rows = value["daily"].as_array().expect("daily rows");
    let day = rows
        .iter()
        .find(|row| row["date"] == json!("1970-01-03"))
        .expect("the local day the activity lands on");
    assert_eq!(day["notes"], json!(1));
    assert_eq!(day["commits"], json!(1));
    assert_eq!(day["tasksCreated"], json!(1));
    assert_eq!(day["tasksCompletedByUpdate"], json!(1));
}

#[test]
fn completion_is_dated_by_update_and_only_for_tasks_that_are_done() {
    let snapshot = snapshot(
        Vec::new(),
        vec![
            // Done, and last touched two days after it was created.
            task(1, 0, TaskStatus::Done, 10 * DAY, 12 * DAY),
            // Touched on the same day, but not done: it must not be counted.
            task(2, 0, TaskStatus::Doing, 10 * DAY, 12 * DAY),
        ],
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    let value = build(&snapshot, 4, 20 * DAY);
    let rows = value["daily"].as_array().expect("daily rows");
    let completed: i64 = rows
        .iter()
        .filter_map(|row| row["tasksCompletedByUpdate"].as_i64())
        .sum();
    assert_eq!(completed, 1, "only the done task counts");

    let on_update_day = rows
        .iter()
        .find(|row| row["date"] == json!("1970-01-13"))
        .expect("the day of the update");
    assert_eq!(on_update_day["tasksCompletedByUpdate"], json!(1));
    assert_eq!(on_update_day["tasksCreated"], json!(0));
}

#[test]
fn the_burn_up_runs_from_the_whole_project_not_from_the_window() {
    let snapshot = snapshot(
        Vec::new(),
        vec![
            // Created long before the window and already done.
            task(1, 0, TaskStatus::Done, 0, 0),
            task(2, 0, TaskStatus::Done, 0, 0),
            // Created inside the window, still open.
            task(3, 0, TaskStatus::Todo, 21 * DAY, 21 * DAY),
        ],
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    let value = build(&snapshot, 2, 25 * DAY);
    let weeks = value["cumulative"].as_array().expect("weekly rows");
    let first = weeks.first().expect("a first week");
    let last = weeks.last().expect("a last week");

    assert_eq!(
        first["completedByUpdate"],
        json!(2),
        "work finished before the window is already on the board",
    );
    assert_eq!(last["created"], json!(3));
    assert_eq!(last["completedByUpdate"], json!(2));

    // The series only ever climbs.
    let mut previous = 0;
    for week in weeks {
        let created = week["created"].as_i64().expect("created count");
        assert!(created >= previous, "cumulative series must not fall");
        previous = created;
    }
}

#[test]
fn lead_time_buckets_by_elapsed_days_and_drops_impossible_spans() {
    let snapshot = snapshot(
        Vec::new(),
        vec![
            task(1, 0, TaskStatus::Done, 0, 0),              // under a day
            task(2, 0, TaskStatus::Done, 0, 2 * DAY),        // 1-3 days
            task(3, 0, TaskStatus::Done, 0, 5 * DAY),        // 3-7 days
            task(4, 0, TaskStatus::Done, 0, 40 * DAY),       // over 28 days
            task(5, 0, TaskStatus::Todo, 0, 40 * DAY),       // not done
            task(6, 0, TaskStatus::Done, 10 * DAY, 2 * DAY), // updated before created
        ],
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    let value = build(&snapshot, 8, 60 * DAY);
    let lead = &value["leadTime"];

    assert_eq!(
        lead["counted"],
        json!(4),
        "open and impossible spans dropped"
    );
    let buckets = lead["buckets"].as_array().expect("buckets");
    assert_eq!(buckets[0], json!({ "label": "under a day", "count": 1 }));
    assert_eq!(buckets[1], json!({ "label": "1-3 days", "count": 1 }));
    assert_eq!(buckets[2], json!({ "label": "3-7 days", "count": 1 }));
    assert_eq!(buckets[3], json!({ "label": "7-14 days", "count": 0 }));
    assert_eq!(buckets[4], json!({ "label": "14-28 days", "count": 0 }));
    assert_eq!(buckets[5], json!({ "label": "over 28 days", "count": 1 }));
    assert_eq!(lead["medianDays"], json!(3), "median of 0, 2, 5 and 40");
}

#[test]
fn plan_progress_counts_only_that_plan_s_tasks() {
    let snapshot = snapshot(
        vec![
            plan(1, "shipping", PlanStatus::Active),
            plan(2, "later", PlanStatus::Done),
        ],
        vec![
            task(1, 1, TaskStatus::Done, 0, 0),
            task(2, 1, TaskStatus::Blocked, 0, 0),
            task(3, 1, TaskStatus::Todo, 0, 0),
            task(4, 2, TaskStatus::Done, 0, 0),
        ],
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    let value = build(&snapshot, 1, 10 * DAY);
    let plans = value["plans"].as_array().expect("plan rows");

    assert_eq!(
        plans[0],
        json!({
            "id": 1,
            "title": "shipping",
            "status": "active",
            "total": 3,
            "done": 1,
            "blocked": 1,
        }),
    );
    assert_eq!(plans[1]["total"], json!(1));
    assert_eq!(plans[1]["status"], json!("done"));
}

#[test]
fn issues_split_by_severity_and_open_ones_carry_their_age_newest_last() {
    let snapshot = snapshot(
        Vec::new(),
        Vec::new(),
        vec![
            issue(1, Severity::Critical, IssueStatus::Open, 0),
            issue(2, Severity::Low, IssueStatus::Open, 8 * DAY),
            issue(3, Severity::Low, IssueStatus::Closed, 0),
        ],
        Vec::new(),
        Vec::new(),
    );
    let value = build(&snapshot, 2, 10 * DAY);

    assert_eq!(
        value["issues"]["bySeverity"],
        json!([
            { "severity": "critical", "open": 1, "closed": 0 },
            { "severity": "high", "open": 0, "closed": 0 },
            { "severity": "medium", "open": 0, "closed": 0 },
            { "severity": "low", "open": 1, "closed": 1 },
        ]),
    );

    let ages = value["issues"]["openAges"].as_array().expect("open ages");
    assert_eq!(ages.len(), 2, "closed issues have no age to report");
    assert_eq!(
        ages[0],
        json!({ "id": 1, "severity": "critical", "days": 10 })
    );
    assert_eq!(ages[1], json!({ "id": 2, "severity": "low", "days": 2 }));
    assert_eq!(value["totals"]["issuesOpen"], json!(2));
    assert_eq!(value["totals"]["issuesClosed"], json!(1));
}

#[test]
fn the_punchcard_places_activity_by_local_weekday_and_hour_inside_the_window() {
    // 1970-01-01 was a Thursday, which is weekday index 3 counting from Monday.
    let thursday_ten = 9 * 60 * 60 + 30 * 60;
    let snapshot = snapshot(
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![note(1, thursday_ten), note(2, thursday_ten)],
        vec![commit(1, thursday_ten)],
    );
    let now = OffsetDateTime::from_unix_timestamp(3 * DAY).expect("valid instant");
    let value = insights_at(&snapshot, 1, now, |_| {
        UtcOffset::from_hms(1, 0, 0).expect("valid offset")
    });

    assert_eq!(
        value["punchcard"],
        json!([{ "weekday": 3, "hour": 10, "count": 3 }]),
        "an hour ahead of UTC moves 09:30 into the ten o'clock column",
    );
}

#[test]
fn the_punchcard_ignores_activity_older_than_the_window() {
    let snapshot = snapshot(
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![note(1, 0), note(2, 30 * DAY)],
        Vec::new(),
    );
    let value = build(&snapshot, 1, 31 * DAY);
    let cells = value["punchcard"].as_array().expect("punchcard cells");
    let counted: i64 = cells.iter().filter_map(|cell| cell["count"].as_i64()).sum();
    assert_eq!(counted, 1, "only the note inside the window is placed");
}

#[test]
fn totals_describe_the_whole_project_regardless_of_the_window() {
    let snapshot = snapshot(
        vec![
            plan(1, "one", PlanStatus::Done),
            plan(2, "two", PlanStatus::Active),
        ],
        vec![
            task(1, 1, TaskStatus::Done, 0, 0),
            task(2, 1, TaskStatus::Blocked, 0, 0),
        ],
        vec![issue(1, Severity::High, IssueStatus::Open, 0)],
        vec![note(1, 0)],
        vec![commit(1, 0)],
    );
    // A one-week window ending far after every record: the totals still count
    // everything, because they describe the project, not the window.
    let value = build(&snapshot, 1, 400 * DAY);

    assert_eq!(
        value["totals"],
        json!({
            "tasks": 2,
            "tasksDone": 1,
            "tasksBlocked": 1,
            "plans": 2,
            "plansDone": 1,
            "milestones": 0,
            "notes": 1,
            "commits": 1,
            "issuesOpen": 1,
            "issuesClosed": 0,
        }),
    );
}

#[test]
fn milestones_are_counted_even_though_they_carry_no_series() {
    let snapshot = ProjectSnapshot::new(
        meta(),
        vec![Milestone {
            id: 1,
            title: "ship".to_owned(),
            status: MilestoneStatus::Open,
            due: Timestamp::Zero,
            order: 1,
            created_at: at(0),
            updated_at: at(0),
            actor: None,
            ulid: None,
        }],
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    let value = build(&snapshot, 1, 10 * DAY);
    assert_eq!(value["totals"]["milestones"], json!(1));
}
