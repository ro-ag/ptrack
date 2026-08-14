use crate::test_support::{milestone, plan, task};
use crate::{
    CapabilityKind, Counts, IssueStatus, MemoryKind, MilestoneStatus, NoteTarget, PlanStatus,
    RecordKind, Severity, TaskStatus, Timestamp,
};

#[test]
fn persistent_enums_expose_and_parse_exact_go_names() {
    assert_eq!(PlanStatus::Archived.as_str(), "archived");
    assert_eq!(TaskStatus::from_name("doing"), Some(TaskStatus::Doing));
    assert_eq!("task".parse(), Ok(NoteTarget::Task));
    assert_eq!("".parse(), Ok(MemoryKind::Legacy));
    assert_eq!("done".parse(), Ok(MilestoneStatus::Done));
    assert_eq!("closed".parse(), Ok(IssueStatus::Closed));
    assert_eq!("critical".parse(), Ok(Severity::Critical));
    assert_eq!("ssh".parse(), Ok(CapabilityKind::Ssh));
    assert_eq!("memory_writeback".parse(), Ok(RecordKind::MemoryWriteback));
    assert_eq!(TaskStatus::Blocked.to_string(), "blocked");

    let error = "Doing"
        .parse::<TaskStatus>()
        .expect_err("case must be exact");
    assert_eq!(error.enum_name(), "TaskStatus");
    assert_eq!(error.value(), "Doing");
    assert_eq!(error.to_string(), "invalid TaskStatus value \"Doing\"");
}

#[test]
fn open_status_and_order_helpers_match_the_go_model() {
    assert!(TaskStatus::Todo.is_open());
    assert!(TaskStatus::Doing.is_open());
    assert!(TaskStatus::Blocked.is_open());
    assert!(!TaskStatus::Done.is_open());

    assert_eq!(plan(1, "p", PlanStatus::Active, 0, 7).ord(), 7);
    assert_eq!(task(1, 1, "t", TaskStatus::Todo, 8).ord(), 8);
    assert_eq!(milestone(1, 9).ord(), 9);
    assert_eq!(Counts::default().tasks_open, 0);
}

#[test]
fn stored_dates_use_the_persisted_fixed_offset() {
    let date = |seconds, offset_seconds| {
        Timestamp::Fixed {
            seconds,
            nanoseconds: 999_999_999,
            offset_seconds,
        }
        .stored_date()
        .expect("fixed timestamp has a date")
        .to_string()
    };

    assert_eq!(date(0, 0), "1970-01-01");
    assert_eq!(date(0, -1), "1969-12-31");
    assert_eq!(date(86_399, 1), "1970-01-02");
    assert_eq!(date(951_782_400, 0), "2000-02-29");
    assert_eq!(date(-62_198_755_200, 0), "-0001-01-01");
    assert_eq!(Timestamp::Zero.stored_date(), None);
}
