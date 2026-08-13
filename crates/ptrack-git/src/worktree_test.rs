use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::model::ExistingWorktree;
use crate::runner::CancellationToken;
use crate::snapshot::RepositoryService;
use crate::test_support::{FakeRunner, canonical, run_git, sha};
use crate::worktree::parse_worktree_list;

#[test]
fn parse_worktree_list_is_content_free_skips_valued_prunable_and_rejects_malformed() {
    let base = temp_directory("ptrack-git-worktree-list");
    let first = base.join("first");
    let second = base.join("second");
    std::fs::create_dir(&first).expect("create first worktree");
    std::fs::create_dir(&second).expect("create second worktree");
    let output = format!(
        "worktree {}\0HEAD {}\0branch refs/heads/main\0\0\
         worktree {}\0HEAD {}\0detached\0\0\
         worktree /stale\0HEAD {}\0prunable gitdir file points to non-existent location\0\0",
        first.display(),
        sha('a'),
        second.display(),
        sha('b'),
        sha('c')
    );
    let (worktrees, bounds) = parse_worktree_list(output.as_bytes()).expect("parse worktrees");
    assert_eq!(
        worktrees,
        [
            ExistingWorktree {
                root: canonical(&first).to_string_lossy().into_owned(),
                branch: "main".to_owned(),
                head: sha('a')
            },
            ExistingWorktree {
                root: canonical(&second).to_string_lossy().into_owned(),
                head: sha('b'),
                ..ExistingWorktree::default()
            }
        ]
    );
    assert_eq!((bounds.shown, bounds.total, bounds.more), (2, 2, 0));
    assert!(parse_worktree_list(b"worktree relative\0HEAD bad\0\0").is_err());
    std::fs::remove_dir_all(base).expect("remove worktree list directory");
}

#[test]
fn inspect_worktree_requires_shared_identity_containment_and_membership() {
    let base = temp_directory("ptrack-git-worktree-inspection");
    let project = base.join("project");
    let project_git = project.join(".git");
    let sibling = base.join("sibling");
    let sibling_git = project_git.join("worktrees/sibling");
    for path in [&project_git, &sibling, &sibling_git] {
        std::fs::create_dir_all(path).expect("create identity directory");
    }
    let head = sha('a');
    let identity = |root: &std::path::Path, git_dir: &std::path::Path, common: &std::path::Path| {
        format!(
            "true\n{}\n{}\n{}\nfalse\n{head}\n",
            root.display(),
            git_dir.display(),
            common.display()
        )
    };
    let runner = Arc::new(FakeRunner::default());
    runner.output(
        &format!("{}|rev-parse", project.display()),
        identity(&project, &project_git, &project_git),
    );
    runner.output(
        &format!("{}|symbolic-ref", canonical(&project).display()),
        b"main\n".to_vec(),
    );
    runner.output(
        &format!("{}|rev-parse", sibling.display()),
        identity(&sibling, &sibling_git, &project_git),
    );
    runner.output(
        &format!("{}|symbolic-ref", canonical(&sibling).display()),
        b"feature\n".to_vec(),
    );
    runner.output(
        &format!("{}|worktree", project.display()),
        format!(
            "worktree {}\0HEAD {head}\0branch refs/heads/main\0\0\
             worktree {}\0HEAD {head}\0branch refs/heads/feature\0\0",
            project.display(),
            sibling.display()
        ),
    );
    let identity = RepositoryService::with_runner_and_clock(runner, || 0)
        .inspect_worktree(&CancellationToken::new(), &project, &sibling)
        .expect("inspect registered sibling");
    assert_eq!(identity.root, canonical(&sibling).to_string_lossy());
    assert_eq!(identity.branch, "feature");
    assert_eq!(identity.head, head);
    assert!(identity.linked);

    std::fs::remove_dir_all(base).expect("remove worktree identity directory");
}

#[test]
fn inspect_worktree_against_real_disposable_linked_worktree() {
    if std::process::Command::new("git")
        .arg("--version")
        .status()
        .is_err()
    {
        return;
    }
    let base = temp_directory("ptrack-git-real-worktree");
    let project = base.join("project");
    let sibling = base.join("sibling");
    std::fs::create_dir(&project).expect("create project");
    run_git(&project, &["init", "-q"]);
    run_git(&project, &["config", "user.name", "P Track"]);
    run_git(&project, &["config", "user.email", "ptrack@example.test"]);
    std::fs::write(project.join("tracked.txt"), b"one\n").expect("write tracked file");
    run_git(&project, &["add", "tracked.txt"]);
    run_git(&project, &["commit", "-q", "-m", "initial"]);
    run_git(
        &project,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feature",
            sibling.to_str().expect("sibling utf8"),
        ],
    );
    let identity = RepositoryService::new()
        .inspect_worktree(&CancellationToken::new(), &project, &sibling)
        .expect("inspect real linked worktree");
    assert_eq!(identity.root, canonical(&sibling).to_string_lossy());
    assert_eq!(identity.branch, "feature");
    assert!(identity.linked);
    assert_eq!(identity.head.len(), 40);
    std::fs::remove_dir_all(base).expect("remove real worktree repository");
}

fn temp_directory(prefix: &str) -> std::path::PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let directory = std::env::temp_dir().join(format!(
        "{prefix}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&directory).expect("create test directory");
    directory
}
