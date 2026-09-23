use crate::overview::{read_global_overview, write_project_summary};
use ptrack_core::{Meta, ProjectRef, ProjectSnapshot, Task, TaskStatus, Timestamp};
use ptrack_store::ActiveGenerationProject;
use std::fs;
use std::path::PathBuf;

struct Home(PathBuf);
impl Home {
    fn new() -> Self {
        let mut bytes = [0; 16];
        getrandom::fill(&mut bytes).unwrap();
        let path = std::env::temp_dir().join(format!(
            "ptrack-overview-{:032x}",
            u128::from_le_bytes(bytes)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn fixture() -> (ProjectSnapshot, ProjectRef, ActiveGenerationProject) {
    let stamp = Timestamp::Fixed {
        seconds: 123,
        nanoseconds: 0,
        offset_seconds: 0,
    };
    let mut snapshot = ProjectSnapshot::new(
        Meta {
            goal: String::new(),
            summary: String::new(),
            active_plan: 0,
            created_at: stamp,
            updated_at: stamp,
            format_version: 1,
            last_write_version: String::new(),
            active_plans: vec![],
            actors: vec![],
            stack: None,
            scratchpad: None,
            summary_updated_at: None,
        },
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
    );
    snapshot.tasks.push(Task {
        id: 1,
        plan_id: 1,
        title: "Done task edited later".into(),
        status: TaskStatus::Done,
        order: 0,
        created_at: Timestamp::Zero,
        updated_at: stamp,
        hold_reason: None,
        actor: None,
        ulid: None,
        deps: vec![],
    });
    let project = ProjectRef {
        name: "Example".into(),
        path: "/not-mounted/project".into(),
        last_seen: stamp,
        stack: None,
    };
    let binding = ActiveGenerationProject {
        root: project.path.clone(),
        database_id: "identity".into(),
        path: "/not-mounted/project/.ptrack/ptrack.redb".into(),
    };
    (snapshot, project, binding)
}

#[test]
fn cache_roundtrip_replaces_and_aggregates_without_project_access() {
    let home = Home::new();
    let (mut snapshot, project, binding) = fixture();
    write_project_summary(&home.0, project.path.as_ref(), "identity", &snapshot).unwrap();
    let overview = read_global_overview(
        &home.0,
        std::slice::from_ref(&project),
        std::slice::from_ref(&binding),
    );
    assert_eq!(overview.counts.done_tasks, 1);
    assert_eq!(overview.projects[0].activity[0].updated_at, 123);
    snapshot.tasks[0].status = TaskStatus::Todo;
    write_project_summary(&home.0, project.path.as_ref(), "identity", &snapshot).unwrap();
    let overview = read_global_overview(&home.0, &[project], &[binding]);
    assert_eq!(overview.counts.done_tasks, 0);
    assert_eq!(overview.counts.open_tasks, 1);
}

#[test]
fn missing_corrupt_unregistered_and_replaced_databases_are_not_counted() {
    let home = Home::new();
    let (snapshot, project, mut binding) = fixture();
    assert_eq!(
        read_global_overview(&home.0, std::slice::from_ref(&project), &[]).summarized_projects,
        0
    );
    write_project_summary(&home.0, project.path.as_ref(), "identity", &snapshot).unwrap();
    assert_eq!(
        read_global_overview(&home.0, &[], std::slice::from_ref(&binding)).tracked_projects,
        0
    );
    binding.database_id = "replacement".into();
    assert_eq!(
        read_global_overview(
            &home.0,
            std::slice::from_ref(&project),
            std::slice::from_ref(&binding)
        )
        .summarized_projects,
        0
    );
    binding.database_id = "identity".into();
    let cache = fs::read_dir(home.0.join("overview"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    fs::write(cache, b"bad JSON").unwrap();
    let overview = read_global_overview(&home.0, &[project], &[binding]);
    assert_eq!(overview.tracked_projects, 1);
    assert_eq!(overview.summarized_projects, 0);
}

#[cfg(unix)]
#[test]
fn cache_directory_symlink_is_rejected() {
    let home = Home::new();
    let target = Home::new();
    let (snapshot, project, binding) = fixture();
    std::os::unix::fs::symlink(&target.0, home.0.join("overview")).unwrap();
    assert!(write_project_summary(&home.0, project.path.as_ref(), "identity", &snapshot).is_err());
    assert_eq!(
        read_global_overview(&home.0, &[project], &[binding]).summarized_projects,
        0
    );
    assert!(fs::read_dir(&target.0).unwrap().next().is_none());
}

#[test]
fn out_of_range_cache_dates_are_uncovered_instead_of_reaching_the_ui() {
    let home = Home::new();
    let (snapshot, project, binding) = fixture();
    write_project_summary(&home.0, project.path.as_ref(), "identity", &snapshot).unwrap();
    let cache = fs::read_dir(home.0.join("overview"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let original: serde_json::Value = serde_json::from_slice(&fs::read(&cache).unwrap()).unwrap();
    for invalid in [i64::MIN, i64::MAX] {
        for activity_date in [false, true] {
            let mut corrupted = original.clone();
            if activity_date {
                corrupted["activity"][0]["updatedAt"] = serde_json::json!(invalid);
            } else {
                corrupted["syncedAt"] = serde_json::json!(invalid);
            }
            fs::write(&cache, serde_json::to_vec(&corrupted).unwrap()).unwrap();
            let overview = read_global_overview(
                &home.0,
                std::slice::from_ref(&project),
                std::slice::from_ref(&binding),
            );
            assert_eq!(overview.tracked_projects, 1);
            assert_eq!(overview.summarized_projects, 0);
            assert!(overview.projects.is_empty());
        }
    }
    fs::write(&cache, serde_json::to_vec(&original).unwrap()).unwrap();
    assert_eq!(
        read_global_overview(&home.0, &[project], &[binding]).summarized_projects,
        1
    );
}

#[cfg(unix)]
#[test]
fn fifo_cache_is_uncovered_without_waiting_for_a_writer() {
    let home = Home::new();
    let (snapshot, project, binding) = fixture();
    write_project_summary(&home.0, project.path.as_ref(), "identity", &snapshot).unwrap();
    let cache = fs::read_dir(home.0.join("overview"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    fs::remove_file(&cache).unwrap();
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&cache)
            .status()
            .unwrap()
            .success()
    );
    let overview = read_global_overview(&home.0, &[project], &[binding]);
    assert_eq!(overview.tracked_projects, 1);
    assert_eq!(overview.summarized_projects, 0);
}
