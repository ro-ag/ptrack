use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use ptrack_core::{NoteTarget, PlanStatus, TaskStatus};
use ptrack_store::{
    ActiveBinding, GlobalStore, PinnedProjectDirectory, ProjectStore, StoreError, StoreKind,
};

use crate::{
    AppError, ApplicationPort, INVALID_HOLD_PREFIX, InitRequest, LocalApplication, Mutation,
    MutationResult, PlanLifecycleOutcome, PlanLifecycleRequest, ProjectEndpoint, WorkspaceBindings,
};
#[cfg(unix)]
use crate::{GuideAction, HookAction, HookResult};

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(name: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("ptrack-app-{name}-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&path).expect("create test directory");
        ptrack_store::protect_private_directory(&path).expect("protect test directory");
        Self(std::fs::canonicalize(path).expect("canonical test directory"))
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn binding(path: &Path, kind: StoreKind, id: &str) -> ActiveBinding {
    ActiveBinding {
        generation: 9,
        database_id: id.to_owned(),
        kind,
        canonical_path: path.to_path_buf(),
    }
}

fn configured(test: &TestDirectory, create_project: bool) -> (LocalApplication, ProjectEndpoint) {
    let root = test.0.join("project");
    let home = test.0.join("home");
    std::fs::create_dir_all(root.join(".ptrack")).expect("project directory");
    ptrack_store::protect_private_directory(&root.join(".ptrack"))
        .expect("protect project directory");
    std::fs::create_dir_all(&home).expect("home directory");
    let project_database = root.join(".ptrack/ptrack.redb");
    let global_database = home.join("global.redb");
    let project_binding = binding(&project_database, StoreKind::Project, "project-9");
    let global_binding = binding(&global_database, StoreKind::Global, "global-9");
    drop(
        GlobalStore::create_new(&global_database, global_binding.clone())
            .expect("create global store"),
    );
    if create_project {
        drop(
            ProjectStore::create_new(&project_database, project_binding.clone(), "test")
                .expect("create project store"),
        );
    }
    let endpoint = ProjectEndpoint {
        root: root.clone(),
        database: project_database,
        binding: project_binding,
    };
    let application = LocalApplication::new(WorkspaceBindings {
        current_dir: root,
        project: Some(endpoint.clone()),
        global_database,
        global_binding,
        global_home: home,
        writer_version: "test".to_owned(),
    });
    (application, endpoint)
}

#[test]
fn agent_observation_requires_the_active_project_host() {
    let directory = TestDirectory::new("ptrack-app-agent-observation");
    let (mut application, _) = configured(&directory, true);
    assert_eq!(
        application.agent_runs().unwrap_err().to_string(),
        "no active agent coordination host for this project"
    );
    assert_eq!(
        application.agent_inbox().unwrap_err().to_string(),
        "no active agent coordination host for this project"
    );
}

#[test]
fn setting_the_rolling_summary_refuses_a_note_dump() {
    let directory = TestDirectory::new("summary-bound");
    let (mut application, _) = configured(&directory, true);

    let narrative = "Stack discovery landed and the release is staged. \
                     Acceptance is green; the tag is not pushed yet.";
    application
        .mutate(Mutation::SetSummary(narrative.to_owned()))
        .expect("a handoff narrative stores");
    assert_eq!(
        application.snapshot().unwrap().meta.summary,
        narrative,
        "the stored summary is the text that was set"
    );

    let dump = "x".repeat(ptrack_core::MAX_SUMMARY_BYTES + 1);
    let error = application
        .mutate(Mutation::SetSummary(dump))
        .expect_err("an oversized summary is refused");
    let message = error.to_string();
    assert!(
        message.starts_with("the rolling summary is") && message.contains("2-4 sentences"),
        "the refusal has to be actionable mid-run: {message}"
    );
}

#[test]
fn operations_reopen_and_drop_the_store() {
    let directory = TestDirectory::new("reopen");
    let (mut application, endpoint) = configured(&directory, true);
    let result = application
        .mutate(Mutation::AddPlan {
            title: "one".to_owned(),
            milestone_id: 0,
        })
        .expect("add plan");
    let MutationResult::Plan(plan) = result else {
        panic!("wrong mutation result");
    };
    assert_eq!(application.snapshot().expect("snapshot").plans.len(), 1);

    // A successful open while the application object remains alive proves the
    // preceding service operation retained no redb handle/lock.
    let concurrent =
        ProjectStore::open_existing(&endpoint.database, &endpoint.binding, "concurrent")
            .expect("store was not held idle");
    concurrent
        .set_plan_status(plan.id, PlanStatus::Done)
        .expect("concurrent write");
    drop(concurrent);
    assert_eq!(
        application.snapshot().expect("reload").plans[0].status,
        PlanStatus::Done
    );
}

#[test]
fn hold_mutations_reach_the_store_and_keep_the_underlying_status() {
    let directory = TestDirectory::new("hold");
    let (mut application, _) = configured(&directory, true);
    let MutationResult::Plan(plan) = application
        .mutate(Mutation::AddPlan {
            title: "one".to_owned(),
            milestone_id: 0,
        })
        .expect("add plan")
    else {
        panic!("wrong mutation result");
    };
    let MutationResult::Task(task) = application
        .mutate(Mutation::AddTask {
            plan_id: plan.id,
            title: "work".to_owned(),
        })
        .expect("add task")
    else {
        panic!("wrong mutation result");
    };

    application
        .mutate(Mutation::SetTaskStatus {
            id: task.id,
            status: TaskStatus::Doing,
        })
        .expect("start task");
    application
        .mutate(Mutation::SetTaskHold {
            id: task.id,
            reason: Some("waiting on review".to_owned()),
        })
        .expect("hold task");
    application
        .mutate(Mutation::SetPlanHold {
            id: plan.id,
            reason: Some("paused".to_owned()),
        })
        .expect("hold plan");

    let snapshot = application.snapshot().expect("snapshot");
    assert_eq!(
        snapshot.tasks[0].hold_reason.as_deref(),
        Some("waiting on review")
    );
    assert_eq!(snapshot.tasks[0].status, TaskStatus::Doing);
    assert_eq!(snapshot.plans[0].hold_reason.as_deref(), Some("paused"));
    assert_eq!(snapshot.plans[0].status, PlanStatus::Active);

    application
        .mutate(Mutation::SetTaskStatus {
            id: task.id,
            status: TaskStatus::Done,
        })
        .expect("finish task");
    let error = application
        .mutate(Mutation::SetTaskHold {
            id: task.id,
            reason: Some("too late".to_owned()),
        })
        .expect_err("a done task cannot be put on hold");
    assert_eq!(
        error.to_string(),
        format!(
            "{INVALID_HOLD_PREFIX}task #{} is done and cannot be put on hold",
            task.id
        )
    );

    application
        .mutate(Mutation::SetTaskHold {
            id: task.id,
            reason: None,
        })
        .expect("resume task");
    application
        .mutate(Mutation::SetPlanHold {
            id: plan.id,
            reason: None,
        })
        .expect("resume plan");
    let snapshot = application.snapshot().expect("snapshot");
    assert!(snapshot.tasks[0].hold_reason.is_none());
    assert!(snapshot.plans[0].hold_reason.is_none());
}

/// The CLI strips [`INVALID_HOLD_PREFIX`] off an [`AppError`] to show the
/// store's own sentence. Pin the constant against the real `StoreError`
/// rendering so a reworded `Display` fails here instead of leaking the layer
/// prefix to a person.
#[test]
fn the_hold_prefix_constant_is_what_the_store_error_actually_renders() {
    let error = AppError::from(StoreError::InvalidHold(
        "task #1 is done and cannot be put on hold".to_owned(),
    ));
    assert_eq!(
        error.to_string().strip_prefix(INVALID_HOLD_PREFIX),
        Some("task #1 is done and cannot be put on hold")
    );
}

#[test]
fn initialize_uses_the_explicit_binding_and_installs_no_ambient_authority() {
    let directory = TestDirectory::new("initialize");
    let (mut application, endpoint) = configured(&directory, false);
    let result = application
        .initialize(InitRequest {
            root: Some(endpoint.root.clone()),
            goal: "ship".to_owned(),
            force: false,
            no_guide: true,
        })
        .expect("initialize");
    assert_eq!(result.database, endpoint.database);
    assert!(!result.already_initialized);
    assert!(result.guide_files.is_empty());
    assert_eq!(application.snapshot().expect("snapshot").meta.goal, "ship");
}

#[cfg(unix)]
#[test]
fn guide_install_rejects_a_symbolic_link_destination() {
    use std::os::unix::fs::symlink;

    let directory = TestDirectory::new("guide-link");
    let (mut application, endpoint) = configured(&directory, true);
    let outside = directory.0.join("outside");
    std::fs::write(&outside, "private").expect("outside file");
    symlink(&outside, endpoint.root.join("AGENTS.md")).expect("guide link");
    let error = application
        .guide(GuideAction::Install)
        .expect_err("symlink must fail");
    assert_eq!(error.to_string(), "project-guide-preview-stale");
    assert_eq!(
        std::fs::read_to_string(outside).expect("outside unchanged"),
        "private"
    );
}

#[cfg(unix)]
#[test]
fn guide_refresh_preserves_existing_mode() {
    use std::os::unix::fs::PermissionsExt;

    let directory = TestDirectory::new("guide-mode");
    let (mut application, endpoint) = configured(&directory, true);
    let guide = endpoint.root.join("AGENTS.md");
    std::fs::write(&guide, "private notes\n").expect("guide seed");
    std::fs::set_permissions(&guide, std::fs::Permissions::from_mode(0o600)).expect("private mode");

    application
        .guide(GuideAction::Install)
        .expect("guide install");
    assert_eq!(
        std::fs::metadata(guide)
            .expect("guide metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[cfg(unix)]
#[test]
fn guide_install_uses_the_shared_project_root_publication_lock() {
    let directory = TestDirectory::new("guide-root-lock");
    let (mut application, endpoint) = configured(&directory, true);
    let retained = PinnedProjectDirectory::prepare(&endpoint.root).expect("retain root lock");

    let error = application
        .guide(GuideAction::Install)
        .expect_err("concurrent ptrack publisher must be fenced");
    assert!(error.to_string().contains("busy"));
    assert!(!endpoint.root.join("AGENTS.md").exists());
    assert!(!endpoint.root.join("CLAUDE.md").exists());

    drop(retained);
    application
        .guide(GuideAction::Install)
        .expect("guide install after lock release");
}

#[cfg(unix)]
#[test]
fn guide_install_publishes_through_retained_root_after_path_replacement() {
    let directory = TestDirectory::new("guide-root-replacement");
    let (mut application, endpoint) = configured(&directory, true);
    let moved = directory.0.join("moved-project");
    let root = endpoint.root.clone();
    let root_for_hook = root.clone();
    let moved_for_hook = moved.clone();
    crate::production::set_guide_before_publish_hook(move || {
        std::fs::rename(&root_for_hook, &moved_for_hook).expect("move retained root");
        std::fs::create_dir(&root_for_hook).expect("replacement root");
        std::fs::write(root_for_hook.join("marker"), "replacement\n").expect("replacement marker");
    });

    application
        .guide(GuideAction::Install)
        .expect_err("replaced root path must fail closed");
    assert_eq!(
        std::fs::read_to_string(root.join("marker")).unwrap(),
        "replacement\n"
    );
    assert!(!root.join("AGENTS.md").exists());
    assert!(!root.join("CLAUDE.md").exists());
    assert!(!moved.join("AGENTS.md").exists());
}

#[cfg(unix)]
#[test]
fn hook_operations_reject_links_and_publish_exact_executable_block() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let directory = TestDirectory::new("hook-safe");
    let (mut application, endpoint) = configured(&directory, true);
    git(&endpoint.root, &["init", "-q"]);
    let hooks = endpoint.root.join(".git/hooks");
    std::fs::create_dir_all(&hooks).expect("hooks directory");
    let hook = hooks.join("post-commit");
    let outside = directory.0.join("outside-hook");
    std::fs::write(&outside, "private").expect("outside hook");
    symlink(&outside, &hook).expect("hook link");
    let error = application
        .hook(HookAction::Install)
        .expect_err("linked hook must fail");
    assert!(error.to_string().contains("symbolic link"));
    assert_eq!(
        std::fs::read_to_string(&outside).expect("outside"),
        "private"
    );

    std::fs::remove_file(&hook).expect("remove link");
    let HookResult::Installed { changed, .. } =
        application.hook(HookAction::Install).expect("install hook")
    else {
        panic!("wrong hook result");
    };
    assert!(changed);
    assert_eq!(
        std::fs::read_to_string(&hook).expect("hook text"),
        concat!(
            "#!/bin/sh\n",
            "# ptrack:begin\n",
            "command -v ptrack >/dev/null 2>&1 && ptrack commit record --sha=\"$(git rev-parse HEAD)\" --subject=\"$(git log -1 --pretty=%s)\" >/dev/null 2>&1 || true\n",
            "# ptrack:end\n"
        )
    );
    assert_eq!(
        std::fs::metadata(&hook)
            .expect("hook metadata")
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
}

#[test]
fn plan_lifecycle_delete_previews_then_deletes_with_summary() {
    let test = TestDirectory::new("lifecycle-delete");
    let (mut application, _endpoint) = configured(&test, true);
    let MutationResult::Plan(plan) = application
        .mutate(Mutation::AddPlan {
            title: "Doomed".to_owned(),
            milestone_id: 0,
        })
        .unwrap()
    else {
        panic!("plan result");
    };
    let MutationResult::Task(task) = application
        .mutate(Mutation::AddTask {
            plan_id: plan.id,
            title: "t".to_owned(),
        })
        .unwrap()
    else {
        panic!("task result");
    };
    application
        .mutate(Mutation::AddNote {
            target: NoteTarget::Task,
            target_id: task.id,
            body: "n".to_owned(),
        })
        .unwrap();
    application
        .mutate(Mutation::AddIssue {
            title: "bug".to_owned(),
            body: String::new(),
            severity: None,
            task_id: task.id,
        })
        .unwrap();
    application
        .mutate(Mutation::SetActivePlan(plan.id))
        .unwrap();

    let preview = application
        .plan_lifecycle(PlanLifecycleRequest::DeletePreview { plan_id: plan.id })
        .unwrap();
    let PlanLifecycleOutcome::Preview(summary) = preview else {
        panic!("preview outcome");
    };
    assert_eq!(
        (summary.tasks, summary.notes, summary.issues.len()),
        (1, 1, 1)
    );
    assert!(
        application
            .snapshot()
            .unwrap()
            .plans
            .iter()
            .any(|p| p.id == plan.id)
    );

    let deleted = application
        .plan_lifecycle(PlanLifecycleRequest::Delete { plan_id: plan.id })
        .unwrap();
    let PlanLifecycleOutcome::Deleted(summary) = deleted else {
        panic!("deleted outcome");
    };
    assert_eq!((summary.tasks, summary.notes), (1, 1));
    let snapshot = application.snapshot().unwrap();
    assert!(snapshot.plans.iter().all(|p| p.id != plan.id));
    assert_eq!(snapshot.meta.active_plan, 0);
}

#[test]
fn plan_lifecycle_move_to_current_project_is_refused_pointing_at_rename() {
    let test = TestDirectory::new("lifecycle-move-self");
    let (mut application, _endpoint) = configured(&test, true);
    let MutationResult::Plan(plan) = application
        .mutate(Mutation::AddPlan {
            title: "Stay".to_owned(),
            milestone_id: 0,
        })
        .unwrap()
    else {
        panic!("plan result");
    };
    let error = application
        .plan_lifecycle(PlanLifecycleRequest::Move {
            plan_id: plan.id,
            to: "project".to_owned(),
            rename: None,
        })
        .unwrap_err();
    assert!(error.to_string().contains("ptrack plan rename"));
}

#[test]
fn plan_lifecycle_copy_without_target_requires_rename_and_duplicates_with_it() {
    let test = TestDirectory::new("lifecycle-copy-self");
    let (mut application, _endpoint) = configured(&test, true);
    let MutationResult::Plan(plan) = application
        .mutate(Mutation::AddPlan {
            title: "Original".to_owned(),
            milestone_id: 0,
        })
        .unwrap()
    else {
        panic!("plan result");
    };
    let refusal = application
        .plan_lifecycle(PlanLifecycleRequest::Copy {
            plan_id: plan.id,
            to: None,
            rename: None,
        })
        .unwrap_err();
    assert!(refusal.to_string().contains("--as"));

    let outcome = application
        .plan_lifecycle(PlanLifecycleRequest::Copy {
            plan_id: plan.id,
            to: None,
            rename: Some("Second".to_owned()),
        })
        .unwrap();
    let PlanLifecycleOutcome::Transferred(summary) = outcome else {
        panic!("transfer outcome");
    };
    assert!(!summary.moved);
    assert_eq!(summary.title, "Second");
    let titles: Vec<String> = application
        .snapshot()
        .unwrap()
        .plans
        .iter()
        .map(|p| p.title.clone())
        .collect();
    assert!(titles.contains(&"Original".to_owned()));
    assert!(titles.contains(&"Second".to_owned()));
}

#[test]
fn plan_lifecycle_unknown_target_is_refused_with_projects_hint() {
    let test = TestDirectory::new("lifecycle-unknown-target");
    let (mut application, _endpoint) = configured(&test, true);
    let MutationResult::Plan(plan) = application
        .mutate(Mutation::AddPlan {
            title: "Lost".to_owned(),
            milestone_id: 0,
        })
        .unwrap()
    else {
        panic!("plan result");
    };
    let error = application
        .plan_lifecycle(PlanLifecycleRequest::Move {
            plan_id: plan.id,
            to: "no-such-project".to_owned(),
            rename: None,
        })
        .unwrap_err();
    assert!(error.to_string().contains("ptrack projects"));
}

#[test]
fn plan_lifecycle_ambiguous_target_name_is_refused_but_the_exact_path_resolves() {
    let test = TestDirectory::new("lifecycle-ambiguous");
    let (mut application, endpoint) = configured(&test, true);
    application
        .mutate(Mutation::AddPlan {
            title: "Shared".to_owned(),
            milestone_id: 0,
        })
        .unwrap();

    // Registry names are directory basenames: two different roots can both be
    // called "twin", and picking either by registry order would be a guess.
    let first = test.0.join("a/twin");
    let second = test.0.join("b/twin");
    std::fs::create_dir_all(&first).expect("first twin");
    std::fs::create_dir_all(&second).expect("second twin");
    let global_database = test.0.join("home/global.redb");
    let registry = GlobalStore::open_existing(
        &global_database,
        &binding(&global_database, StoreKind::Global, "global-9"),
    )
    .expect("open registry");
    registry
        .register_project("twin", &first)
        .expect("register first");
    registry
        .register_project("twin", &second)
        .expect("register second");
    drop(registry);

    let error = application
        .plan_lifecycle(PlanLifecycleRequest::Copy {
            plan_id: 1,
            to: Some("twin".to_owned()),
            rename: None,
        })
        .unwrap_err()
        .to_string();
    assert!(error.contains("ambiguous"), "{error}");
    assert!(
        error.contains(&first.to_string_lossy().into_owned()),
        "{error}"
    );
    assert!(
        error.contains(&second.to_string_lossy().into_owned()),
        "{error}"
    );

    // The exact path is unambiguous, so lookup succeeds and the refusal that
    // follows comes from the marker, not from the registry.
    let resolved = application
        .plan_lifecycle(PlanLifecycleRequest::Copy {
            plan_id: 1,
            to: Some(second.to_string_lossy().into_owned()),
            rename: None,
        })
        .unwrap_err()
        .to_string();
    assert!(!resolved.contains("ambiguous"), "{resolved}");
    assert!(!resolved.contains("unknown target project"), "{resolved}");
    assert_ne!(endpoint.root, second);
}

#[test]
fn target_open_failures_are_fail_closed_and_only_stale_schemas_get_the_upgrade_hint() {
    let root = Path::new("/tmp/some-target-project");
    let stale = crate::service::target_open_error(
        root,
        &StoreError::InvalidManifest("activation generation is missing".to_owned()),
    )
    .to_string();
    assert!(stale.contains("/tmp/some-target-project"), "{stale}");
    assert!(stale.contains("upgrade ptrack for that project"), "{stale}");

    // A busy target is a retry-later condition, not a version problem: the
    // upgrade hint would send the caller off to reinstall for nothing.
    let busy = crate::service::target_open_error(root, &StoreError::Busy).to_string();
    assert!(busy.contains("/tmp/some-target-project"), "{busy}");
    assert!(!busy.contains("upgrade ptrack"), "{busy}");
}

fn git(root: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?}");
}

fn add_plan_and_task(application: &mut LocalApplication) -> (u64, u64) {
    let MutationResult::Plan(plan) = application
        .mutate(Mutation::AddPlan {
            title: "Plan".to_owned(),
            milestone_id: 0,
        })
        .unwrap()
    else {
        panic!("plan result");
    };
    let MutationResult::Task(task) = application
        .mutate(Mutation::AddTask {
            plan_id: plan.id,
            title: "Task".to_owned(),
        })
        .unwrap()
    else {
        panic!("task result");
    };
    (plan.id, task.id)
}

#[cfg(unix)]
#[test]
fn hook_install_follows_core_hooks_path_and_status_reports_it() {
    let directory = TestDirectory::new("hook-hooks-path");
    let (mut application, endpoint) = configured(&directory, true);
    git(&endpoint.root, &["init", "-q"]);
    git(&endpoint.root, &["config", "core.hooksPath", ".husky/_"]);
    std::fs::create_dir_all(endpoint.root.join(".husky")).unwrap();

    let HookResult::Installed { path, changed, .. } =
        application.hook(HookAction::Install).unwrap()
    else {
        panic!("wrong hook result");
    };
    assert!(changed);
    assert_eq!(path, endpoint.root.join(".husky/_/post-commit"));
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("# ptrack:begin")
    );
    assert!(!endpoint.root.join(".git/hooks/post-commit").exists());
    assert_eq!(
        application.hook(HookAction::Status).unwrap(),
        HookResult::Status {
            path,
            installed: true
        }
    );
}

#[cfg(unix)]
#[test]
fn hook_install_refuses_a_hooks_path_outside_the_project() {
    let directory = TestDirectory::new("hook-outside");
    let (mut application, endpoint) = configured(&directory, true);
    let shared = directory.0.join("shared-hooks");
    std::fs::create_dir_all(&shared).unwrap();
    git(&endpoint.root, &["init", "-q"]);
    git(
        &endpoint.root,
        &["config", "core.hooksPath", shared.to_str().unwrap()],
    );
    let error = application
        .hook(HookAction::Install)
        .unwrap_err()
        .to_string();
    assert!(error.contains("outside this project"), "{error}");
    assert!(error.contains("ptrack commit record"), "{error}");
    assert!(!shared.join("post-commit").exists());
}

#[cfg(unix)]
#[test]
fn hook_install_refuses_a_foreign_interpreter_and_respects_a_final_exec() {
    let directory = TestDirectory::new("hook-shebang");
    let (mut application, endpoint) = configured(&directory, true);
    git(&endpoint.root, &["init", "-q"]);
    let hook = endpoint.root.join(".git/hooks/post-commit");
    std::fs::create_dir_all(hook.parent().unwrap()).unwrap();

    let python = "#!/usr/bin/env python3\nprint('hi')\n";
    std::fs::write(&hook, python).unwrap();
    let error = application
        .hook(HookAction::Install)
        .unwrap_err()
        .to_string();
    assert!(error.contains("runs python3"), "{error}");
    assert_eq!(std::fs::read_to_string(&hook).unwrap(), python);

    std::fs::write(
        &hook,
        "#!/bin/bash\nset -e\nexec lefthook run post-commit \"$@\"\n",
    )
    .unwrap();
    let HookResult::Installed { warning, .. } = application.hook(HookAction::Install).unwrap()
    else {
        panic!("wrong hook result");
    };
    assert!(warning.unwrap().contains("exec lefthook"));
    let content = std::fs::read_to_string(&hook).unwrap();
    let block = content.find("# ptrack:begin").unwrap();
    let exec = content.find("exec lefthook").unwrap();
    assert!(block < exec, "{content}");
    assert!(content.starts_with("#!/bin/bash\nset -e\n"), "{content}");
}

#[test]
fn hook_reports_a_directory_that_is_not_a_git_repository() {
    let directory = TestDirectory::new("hook-no-git");
    let (mut application, _) = configured(&directory, true);
    let error = application
        .hook(HookAction::Status)
        .unwrap_err()
        .to_string();
    assert!(error.contains("is not a git repository"), "{error}");
}

#[test]
fn commit_records_refuse_anything_but_a_hex_sha() {
    let directory = TestDirectory::new("commit-sha");
    let (mut application, _) = configured(&directory, true);
    for sha in [
        "--output=/tmp/x",
        "HEAD~1",
        "abc",
        "g123456",
        &"a".repeat(65),
    ] {
        let error = application
            .mutate(Mutation::AddCommit {
                sha: sha.to_owned(),
                subject: "x".to_owned(),
                plan_id: 0,
                task_id: 0,
            })
            .unwrap_err()
            .to_string();
        assert!(error.contains("invalid commit sha"), "{sha}: {error}");
    }
    assert!(application.snapshot().unwrap().commits.is_empty());
    application
        .mutate(Mutation::AddCommit {
            sha: "0123abcDEF".to_owned(),
            subject: "x".to_owned(),
            plan_id: 0,
            task_id: 0,
        })
        .unwrap();
    assert!(
        application
            .git_show("--output=/tmp/x", false)
            .unwrap_err()
            .to_string()
            .contains("invalid commit reference")
    );
}

#[test]
fn completing_a_missing_task_fails_and_writes_nothing() {
    let directory = TestDirectory::new("complete-missing");
    let (mut application, _) = configured(&directory, true);
    let error = crate::complete_task(&mut application, 999, Some("done".to_owned()), true)
        .unwrap_err()
        .to_string();
    assert!(error.contains("not found"), "{error}");
    assert!(application.snapshot().unwrap().notes.is_empty());
}

#[test]
fn a_refused_close_leaves_no_orphan_notes() {
    let directory = TestDirectory::new("complete-refused");
    let (mut application, endpoint) = configured(&directory, true);
    let (plan_id, task_id) = add_plan_and_task(&mut application);
    // Someone else claims the plan, so the status change is refused.
    let other = ProjectStore::open_existing(&endpoint.database, &endpoint.binding, "test")
        .unwrap()
        .with_actor(Some(ptrack_store::ActorIdentity {
            id: "0123456789abcdefghjkmnpqrs".to_owned(),
            name: "Teammate".to_owned(),
        }));
    other.use_plan(plan_id, false).unwrap();
    drop(other);

    let error = crate::complete_task(&mut application, task_id, Some("done".to_owned()), true)
        .unwrap_err()
        .to_string();
    assert!(error.contains("claimed by"), "{error}");
    let snapshot = application.snapshot().unwrap();
    assert!(snapshot.notes.is_empty(), "{:?}", snapshot.notes);
    assert_eq!(snapshot.task(task_id).unwrap().status, TaskStatus::Todo);
}

#[test]
fn a_forced_close_commits_its_notes_with_the_status() {
    let directory = TestDirectory::new("complete-forced");
    let (mut application, _) = configured(&directory, true);
    let (_, task_id) = add_plan_and_task(&mut application);
    let result =
        crate::complete_task(&mut application, task_id, Some("wired".to_owned()), true).unwrap();
    assert_eq!(result.closeout_note.unwrap().body, "closeout: wired");
    assert!(
        result
            .override_note
            .unwrap()
            .body
            .starts_with("override: closed via --force (no commit is linked")
    );
    let snapshot = application.snapshot().unwrap();
    assert_eq!(snapshot.task(task_id).unwrap().status, TaskStatus::Done);
    assert_eq!(snapshot.notes.len(), 2);
}

#[test]
fn ui_closes_are_allowed_and_record_the_surface() {
    let directory = TestDirectory::new("ui-close");
    let (mut application, _) = configured(&directory, true);
    let (plan_id, task_id) = add_plan_and_task(&mut application);
    let MutationResult::Task(other) = application
        .mutate(Mutation::AddTask {
            plan_id,
            title: "Other".to_owned(),
        })
        .unwrap()
    else {
        panic!("task result");
    };

    let note = crate::close_task_from_ui(&mut application, crate::UiSurface::Tui, task_id)
        .unwrap()
        .expect("override note");
    assert_eq!(
        note.body,
        "override: closed from TUI without evidence (no closeout summary; no linked commit)"
    );
    assert_eq!(note.target, NoteTarget::Task);

    let note = crate::complete_plan_from_ui(&mut application, crate::UiSurface::Tui, plan_id)
        .unwrap()
        .expect("override note");
    assert_eq!(
        note.body,
        format!(
            "override: plan completed from TUI with 1 open task #{}",
            other.id
        )
    );
    let snapshot = application.snapshot().unwrap();
    assert_eq!(snapshot.plan(plan_id).unwrap().status, PlanStatus::Done);
    assert_eq!(snapshot.task(task_id).unwrap().status, TaskStatus::Done);
}

#[test]
fn next_defers_the_integration_task_until_real_work_is_done() {
    let directory = TestDirectory::new("next-integration");
    let (mut application, _) = configured(&directory, true);
    let MutationResult::Plan(plan) = application
        .mutate(Mutation::AddPlan {
            title: "Plan".to_owned(),
            milestone_id: 0,
        })
        .unwrap()
    else {
        panic!("plan result");
    };
    let MutationResult::Task(integration) = application
        .mutate(Mutation::AddTask {
            plan_id: plan.id,
            title: crate::integration_task_title("ship"),
        })
        .unwrap()
    else {
        panic!("task result");
    };
    let MutationResult::Task(real) = application
        .mutate(Mutation::AddTask {
            plan_id: plan.id,
            title: "Real work".to_owned(),
        })
        .unwrap()
    else {
        panic!("task result");
    };
    application
        .mutate(Mutation::SetActivePlan(plan.id))
        .unwrap();
    let snapshot = application.snapshot().unwrap();
    assert_eq!(
        crate::integration_task_id(&snapshot, plan.id),
        Some(integration.id)
    );
    let view = crate::next_task(&snapshot).unwrap();
    assert_eq!(view.task.unwrap().id, real.id);
}
