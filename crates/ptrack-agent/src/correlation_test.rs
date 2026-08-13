use std::fs;

use super::{
    Association, AssociationTarget, EventCorrelation, Run, discover_repository_root,
    event_correlation_for_run,
};
use crate::test_support::TempDirectory;

#[test]
fn correlation_stamps_only_valid_host_association() {
    let mut run = Run {
        id: "run-1".to_owned(),
        project_root: "/project".to_owned(),
        terminal_id: "terminal-1".to_owned(),
        ..Run::default()
    };
    run.association = Some(Association {
        version: 1,
        project_root: "/project".to_owned(),
        generation: 7,
        live_id: "run-1".to_owned(),
        target: AssociationTarget {
            plan_id: 2,
            task_id: 9,
        },
        revision: 3,
    });
    let value = event_correlation_for_run(&run, Some(std::path::Path::new("/repo")));
    assert_eq!(
        serde_json::to_string(&value).unwrap(),
        r#"{"projectRoot":"/project","repositoryRoot":"/repo","terminalId":"terminal-1","planId":2,"taskId":9,"generation":7,"associationRevision":3}"#
    );
    run.association.as_mut().unwrap().live_id = "other".to_owned();
    assert_eq!(
        event_correlation_for_run(&run, None),
        EventCorrelation {
            project_root: "/project".to_owned(),
            terminal_id: "terminal-1".to_owned(),
            ..EventCorrelation::default()
        }
    );
}

#[test]
fn repository_discovery_accepts_directory_and_worktree_file_markers() {
    let root = TempDirectory::new("ptrack-agent-correlation");
    let nested = root.path().join("a/b");
    fs::create_dir_all(&nested).unwrap();
    fs::write(root.path().join(".git"), "gitdir: elsewhere").unwrap();
    assert_eq!(
        discover_repository_root(&nested).unwrap(),
        fs::canonicalize(root.path()).unwrap()
    );
}
