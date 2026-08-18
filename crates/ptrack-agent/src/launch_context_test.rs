#![allow(clippy::unicode_not_nfc)] // Intentional Go Unicode-folding canaries.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use ptrack_core::{
    Commit, Issue, IssueStatus, MemoryKind, Meta, Note, NoteTarget, Plan, PlanStatus, Severity,
    Task, TaskStatus, Timestamp as CoreTimestamp,
};
use serde_json::Value;

use super::{
    AssociationCatalog, AssociationHost, AssociationPointer, BoundedItems, LaunchContextError,
    LaunchContextStore, MAX_CONTEXT_BYTES, REDACTED_CREDENTIAL, ScanBoundedItems,
    UNTRUSTED_DATA_NOTICE, build_launch_context, contains_potential_credential,
};
use crate::test_support::TempDirectory;

struct Store {
    root: PathBuf,
    meta: Meta,
    plans: BTreeMap<u64, Plan>,
    tasks: BTreeMap<u64, Task>,
    notes: Vec<Note>,
    issues: Vec<Issue>,
    commits: Vec<Commit>,
    more: usize,
}

impl Store {
    fn new(root: PathBuf) -> Self {
        Self {
            root,
            meta: Meta {
                goal: "Ship exact parity".to_owned(),
                summary: String::new(),
                active_plan: 2,
                created_at: CoreTimestamp::Zero,
                updated_at: CoreTimestamp::Zero,
                format_version: 1,
                last_write_version: String::new(),
            },
            plans: BTreeMap::new(),
            tasks: BTreeMap::new(),
            notes: Vec::new(),
            issues: Vec::new(),
            commits: Vec::new(),
            more: 0,
        }
    }
}

impl AssociationCatalog for Store {
    fn validate_plan(&self, plan_id: u64) -> Result<(), String> {
        self.plans
            .contains_key(&plan_id)
            .then_some(())
            .ok_or_else(|| "not found".to_owned())
    }
    fn task_plan(&self, task_id: u64) -> Result<u64, String> {
        self.tasks
            .get(&task_id)
            .map(|task| task.plan_id)
            .ok_or_else(|| "not found".to_owned())
    }
}

impl LaunchContextStore for Store {
    fn project_root(&self) -> Result<PathBuf, String> {
        Ok(self.root.clone())
    }
    fn meta(&self) -> Result<Meta, String> {
        Ok(self.meta.clone())
    }
    fn plan(&self, id: u64) -> Result<Option<Plan>, String> {
        Ok(self.plans.get(&id).cloned())
    }
    fn task(&self, id: u64) -> Result<Option<Task>, String> {
        Ok(self.tasks.get(&id).cloned())
    }
    fn recent_notes(&self, limit: usize) -> Result<BoundedItems<Note>, String> {
        Ok(BoundedItems {
            items: self.notes.iter().take(limit).cloned().collect(),
            more: self.more + self.notes.len().saturating_sub(limit),
        })
    }
    fn open_issues(&self, limit: usize) -> Result<ScanBoundedItems<Issue>, String> {
        Ok(ScanBoundedItems {
            items: self.issues.iter().take(limit).cloned().collect(),
            truncated: self.issues.len() > limit,
        })
    }
    fn recent_commits(&self, limit: usize) -> Result<BoundedItems<Commit>, String> {
        Ok(BoundedItems {
            items: self.commits.iter().take(limit).cloned().collect(),
            more: self.more + self.commits.len().saturating_sub(limit),
        })
    }
}

fn plan(id: u64) -> Plan {
    Plan {
        id,
        title: format!("Plan {id}"),
        status: PlanStatus::Active,
        milestone_id: 0,
        order: 1,
        created_at: CoreTimestamp::Zero,
        updated_at: CoreTimestamp::Zero,
        hold_reason: None,
    }
}

fn task(id: u64, plan_id: u64) -> Task {
    Task {
        id,
        plan_id,
        title: format!("Task {id}"),
        status: TaskStatus::Doing,
        order: 1,
        created_at: CoreTimestamp::Zero,
        updated_at: CoreTimestamp::Zero,
        hold_reason: None,
    }
}

#[test]
fn context_is_exact_bounded_untrusted_json_with_relevant_memory() {
    let root = TempDirectory::new("ptrack-agent-launch-context");
    let canonical = fs::canonicalize(root.path()).unwrap();
    let mut store = Store::new(canonical.clone());
    store.plans.insert(2, plan(2));
    store.plans.insert(3, plan(3));
    store.tasks.insert(9, task(9, 2));
    store.tasks.insert(10, task(10, 3));
    store.notes = vec![
        Note {
            id: 1,
            target: NoteTarget::Project,
            target_id: 0,
            kind: MemoryKind::Decision,
            body: "Project decision".to_owned(),
            created_at: CoreTimestamp::Zero,
        },
        Note {
            id: 2,
            target: NoteTarget::Plan,
            target_id: 2,
            kind: MemoryKind::Legacy,
            body: "Plan memory".to_owned(),
            created_at: CoreTimestamp::Zero,
        },
        Note {
            id: 3,
            target: NoteTarget::Task,
            target_id: 10,
            kind: MemoryKind::Decision,
            body: "unrelated".to_owned(),
            created_at: CoreTimestamp::Zero,
        },
    ];
    store.issues.push(Issue {
        id: 4,
        title: "Open issue".to_owned(),
        body: "Bounded".to_owned(),
        status: IssueStatus::Open,
        severity: Severity::High,
        task_id: 9,
        created_at: CoreTimestamp::Zero,
        updated_at: CoreTimestamp::Zero,
    });
    store.commits.push(Commit {
        id: 5,
        sha: "01234567".to_owned(),
        subject: "Current task".to_owned(),
        plan_id: 2,
        task_id: 9,
        created_at: CoreTimestamp::Zero,
    });
    let host = AssociationHost::new(&canonical, 7, Some(&store)).unwrap();
    let context = build_launch_context(
        Some(&store),
        Some(&host),
        AssociationPointer {
            version: 1,
            plan_id: 2,
            task_id: 9,
        },
    )
    .unwrap();
    assert_eq!(context.version, 1);
    assert_eq!(context.bytes, context.text.len());
    assert!(context.bytes <= MAX_CONTEXT_BYTES);
    let document: Value = serde_json::from_str(&context.text).unwrap();
    assert_eq!(document["notice"], UNTRUSTED_DATA_NOTICE);
    assert_eq!(document["scope"], "task");
    assert_eq!(document["decisions"].as_array().unwrap().len(), 2);
    assert_eq!(document["openIssues"][0]["taskId"], 9);
    assert_eq!(document["recentCommits"][0]["sha"], "01234567");
    assert!(context.text.contains("\n  \"notice\""));
    assert_eq!(
        serde_json::to_value(&context)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>()
            .len(),
        5
    );
}

#[test]
fn a_held_plan_or_task_states_its_hold_in_the_launch_document() {
    let root = TempDirectory::new("ptrack-agent-launch-hold");
    let canonical = fs::canonicalize(root.path()).unwrap();
    let mut store = Store::new(canonical.clone());
    store.plans.insert(2, plan(2));
    store.tasks.insert(9, task(9, 2));
    let pointer = AssociationPointer {
        version: 1,
        plan_id: 2,
        task_id: 9,
    };
    {
        // An unheld plan and task omit the field rather than emitting a null.
        let host = AssociationHost::new(&canonical, 7, Some(&store)).unwrap();
        let context = build_launch_context(Some(&store), Some(&host), pointer).unwrap();
        assert!(!context.text.contains("hold"));
    }

    store.plans.get_mut(&2).unwrap().hold_reason = Some("budget freeze".to_owned());
    store.tasks.get_mut(&9).unwrap().hold_reason = Some("waiting on review".to_owned());
    let host = AssociationHost::new(&canonical, 7, Some(&store)).unwrap();
    let context = build_launch_context(Some(&store), Some(&host), pointer).unwrap();

    let document: Value = serde_json::from_str(&context.text).unwrap();
    assert_eq!(document["plan"]["hold"], "on hold: budget freeze");
    assert_eq!(document["task"]["hold"], "on hold: waiting on review");
    // A hold leaves the status alone, so the status alone cannot carry it.
    assert_eq!(document["task"]["status"], "doing");
}

#[test]
fn context_fences_store_and_redacts_credentials_before_utf8_caps() {
    let root = TempDirectory::new("ptrack-agent-launch-fence");
    let other = TempDirectory::new("ptrack-agent-launch-other");
    let canonical = fs::canonicalize(root.path()).unwrap();
    let mut store = Store::new(canonical.clone());
    store.meta.goal = "safe\ntoken=TOP_SECRET\nsafe".to_owned();
    let host = AssociationHost::new(&canonical, 1, None).unwrap();
    let context = build_launch_context(
        Some(&store),
        Some(&host),
        AssociationPointer {
            version: 1,
            ..AssociationPointer::default()
        },
    )
    .unwrap();
    assert!(context.text.contains(REDACTED_CREDENTIAL));
    assert!(!context.text.contains("TOP_SECRET"));
    assert!(contains_potential_credential("password = secret"));
    assert!(contains_potential_credential("toKen=secret"));
    assert!(contains_potential_credential(
        "-----BEGIN PRIVATE KEY-----\ncanary\n-----END PRIVATE KEY-----"
    ));
    assert!(!contains_potential_credential(
        "secret-store is a benign label"
    ));
    assert_eq!(
        build_launch_context(
            None,
            Some(&host),
            AssociationPointer {
                version: 1,
                ..AssociationPointer::default()
            }
        )
        .unwrap_err(),
        LaunchContextError::StoreRequired
    );
    assert!(matches!(
        build_launch_context(
            Some(&store),
            None,
            AssociationPointer {
                version: 1,
                ..AssociationPointer::default()
            }
        ),
        Err(LaunchContextError::ProjectMismatch { .. })
    ));
    store.root = fs::canonicalize(other.path()).unwrap();
    assert!(matches!(
        build_launch_context(
            Some(&store),
            Some(&host),
            AssociationPointer {
                version: 1,
                ..AssociationPointer::default()
            }
        ),
        Err(LaunchContextError::ProjectMismatch { .. })
    ));
}

#[test]
fn hard_ceiling_is_deterministic_utf8_safe_and_marks_truncation() {
    let root = TempDirectory::new("ptrack-agent-launch-ceiling");
    let canonical = fs::canonicalize(root.path()).unwrap();
    let mut store = Store::new(canonical.clone());
    let huge = "<界\n".repeat(MAX_CONTEXT_BYTES);
    store.meta.goal.clone_from(&huge);
    let mut selected_plan = plan(2);
    selected_plan.title.clone_from(&huge);
    store.plans.insert(2, selected_plan);
    let mut selected_task = task(9, 2);
    selected_task.title.clone_from(&huge);
    store.tasks.insert(9, selected_task);
    for id in 1..=8 {
        store.notes.push(Note {
            id,
            target: NoteTarget::Task,
            target_id: 9,
            kind: MemoryKind::Decision,
            body: huge.clone(),
            created_at: CoreTimestamp::Zero,
        });
    }
    for id in 1..=6 {
        store.issues.push(Issue {
            id,
            title: huge.clone(),
            body: huge.clone(),
            status: IssueStatus::Open,
            severity: Severity::High,
            task_id: 9,
            created_at: CoreTimestamp::Zero,
            updated_at: CoreTimestamp::Zero,
        });
    }
    for id in 1..=8 {
        store.commits.push(Commit {
            id,
            sha: format!("sha-{id}"),
            subject: huge.clone(),
            plan_id: 2,
            task_id: 9,
            created_at: CoreTimestamp::Zero,
        });
    }
    let host = AssociationHost::new(&canonical, 1, Some(&store)).unwrap();
    let pointer = AssociationPointer {
        version: 1,
        plan_id: 2,
        task_id: 9,
    };
    let first = build_launch_context(Some(&store), Some(&host), pointer).unwrap();
    let second = build_launch_context(Some(&store), Some(&host), pointer).unwrap();
    assert_eq!(first, second);
    assert!(first.truncated && first.bytes <= MAX_CONTEXT_BYTES);
    assert!(std::str::from_utf8(first.text.as_bytes()).is_ok());
    assert!(first.text.contains("\\u003c"));
    assert!(!first.text.contains('<'));
    serde_json::from_str::<Value>(&first.text).unwrap();
}

#[test]
fn context_uses_go_json_html_and_line_separator_escaping() {
    let root = TempDirectory::new("ptrack-agent-launch-json-escaping");
    let canonical = fs::canonicalize(root.path()).unwrap();
    let mut store = Store::new(canonical.clone());
    store.meta.goal = "<>&\u{2028}\u{2029}".to_owned();
    let host = AssociationHost::new(&canonical, 1, None).unwrap();
    let context = build_launch_context(
        Some(&store),
        Some(&host),
        AssociationPointer {
            version: 1,
            ..AssociationPointer::default()
        },
    )
    .unwrap();
    assert!(
        context
            .text
            .contains(r#""goal": "\u003c\u003e\u0026\u2028\u2029""#)
    );
    assert_eq!(context.bytes, context.text.len());
    let document: Value = serde_json::from_str(&context.text).unwrap();
    assert_eq!(document["goal"], "<>&\u{2028}\u{2029}");
}
