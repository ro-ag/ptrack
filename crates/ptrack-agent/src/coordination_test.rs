use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::thread;

use super::*;
use crate::coordination::{
    CANDIDATE_LIMIT, INBOX_LIMIT, SUGGESTION_TEXT_BYTES, activity_conflicts,
    bounded_suggestion_text,
};
use crate::test_support::TempDirectory;

const GENERATION: u64 = 7;

struct Catalog;

impl AssociationCatalog for Catalog {
    fn validate_plan(&self, plan_id: u64) -> Result<(), String> {
        matches!(plan_id, 7 | 8)
            .then_some(())
            .ok_or_else(|| "not found".to_owned())
    }

    fn task_plan(&self, task_id: u64) -> Result<u64, String> {
        BTreeMap::from([(9, 7), (10, 7), (11, 8)])
            .get(&task_id)
            .copied()
            .ok_or_else(|| "not found".to_owned())
    }
}

struct FakeStore {
    root: String,
    current: AtomicBool,
    linked: Mutex<Vec<String>>,
    tracking: AtomicI64,
    plans: Mutex<BTreeMap<u64, String>>,
    tasks: Mutex<BTreeMap<u64, CoordinationTaskContext>>,
    decisions: Mutex<Vec<CoordinationDecision>>,
    issues: Mutex<Vec<CoordinationIssue>>,
}

impl CoordinationStore for FakeStore {
    fn current_association(
        &self,
        live_id: &str,
        association: &Association,
    ) -> Option<RuntimeAssociation> {
        (self.current.load(Ordering::SeqCst)
            && association.version == ASSOCIATION_VERSION_V1
            && association.project_root == self.root
            && association.generation == GENERATION
            && association.live_id == live_id
            && association.revision != 0
            && (association.target.task_id == 0 || association.target.plan_id != 0))
            .then_some(RuntimeAssociation {
                plan_id: association.target.plan_id,
                task_id: association.target.task_id,
                revision: association.revision,
            })
    }

    fn linked_commit_shas(&self) -> Vec<String> {
        lock_test(&self.linked).clone()
    }

    fn tracking_started_at(&self) -> Timestamp {
        let seconds = self.tracking.load(Ordering::SeqCst);
        if seconds == 0 {
            Timestamp::ZERO
        } else {
            Timestamp::from_unix_seconds(seconds).add_nanoseconds(500_000_000)
        }
    }

    fn plan_title(&self, plan_id: u64) -> Result<Option<String>, CoordinationError> {
        Ok(lock_test(&self.plans).get(&plan_id).cloned())
    }

    fn task_context(
        &self,
        task_id: u64,
    ) -> Result<Option<CoordinationTaskContext>, CoordinationError> {
        Ok(lock_test(&self.tasks).get(&task_id).cloned())
    }

    fn recent_decisions(
        &self,
        limit: usize,
    ) -> Result<Vec<CoordinationDecision>, CoordinationError> {
        Ok(lock_test(&self.decisions)
            .iter()
            .take(limit)
            .cloned()
            .collect())
    }

    fn open_issues(&self) -> Result<Vec<CoordinationIssue>, CoordinationError> {
        Ok(lock_test(&self.issues).clone())
    }
}

#[derive(Default)]
struct FakeSessions(Mutex<Vec<CoordinationSession>>);

impl CoordinationSessions for FakeSessions {
    fn snapshot(&self, limit: usize) -> (Vec<CoordinationSession>, usize) {
        let sessions = lock_test(&self.0);
        (
            sessions.iter().take(limit).cloned().collect(),
            sessions.len(),
        )
    }
}

struct FakeGit {
    identity: Mutex<WorktreeIdentity>,
    snapshot: Mutex<CoordinationGitSnapshot>,
    inspections: AtomicUsize,
    snapshots: AtomicUsize,
    inspect_block: Mutex<Option<(Arc<Barrier>, Arc<Barrier>)>>,
    snapshot_block: Mutex<Option<(Arc<Barrier>, Arc<Barrier>)>>,
    snapshot_error: AtomicBool,
}

impl FakeGit {
    fn new(root: &Path) -> Self {
        let root = root.to_string_lossy().into_owned();
        let head = "a".repeat(40);
        let identity = WorktreeIdentity {
            root: root.clone(),
            git_dir: format!("{root}/.git"),
            common_git_dir: format!("{root}/.git"),
            branch: "feature".to_owned(),
            head: head.clone(),
            linked: false,
        };
        Self {
            identity: Mutex::new(identity.clone()),
            snapshot: Mutex::new(CoordinationGitSnapshot {
                root: root.clone(),
                git_dir: identity.git_dir.clone(),
                common_git_dir: identity.common_git_dir.clone(),
                branch: identity.branch.clone(),
                head: head.clone(),
                branches: vec![GitBranch {
                    name: "main".to_owned(),
                    head: "b".repeat(40),
                }],
                worktrees: vec![ExistingWorktree {
                    root,
                    branch: "feature".to_owned(),
                    head,
                }],
                worktree_bounds: BoundedSnapshot::new(1, 1),
                ..CoordinationGitSnapshot::default()
            }),
            inspections: AtomicUsize::new(0),
            snapshots: AtomicUsize::new(0),
            inspect_block: Mutex::new(None),
            snapshot_block: Mutex::new(None),
            snapshot_error: AtomicBool::new(false),
        }
    }
}

impl CoordinationGit for FakeGit {
    fn inspect_worktree(
        &self,
        _project_root: &Path,
        _root: &Path,
    ) -> Result<WorktreeIdentity, CoordinationError> {
        self.inspections.fetch_add(1, Ordering::SeqCst);
        if let Some((started, release)) = lock_test(&self.inspect_block).clone() {
            started.wait();
            release.wait();
        }
        Ok(lock_test(&self.identity).clone())
    }

    fn snapshot(&self, _root: &Path) -> Result<CoordinationGitSnapshot, CoordinationError> {
        self.snapshots.fetch_add(1, Ordering::SeqCst);
        if let Some((started, release)) = lock_test(&self.snapshot_block).clone() {
            started.wait();
            release.wait();
        }
        if self.snapshot_error.load(Ordering::SeqCst) {
            return Err(CoordinationError::Message(
                "Git snapshot unavailable".to_owned(),
            ));
        }
        Ok(lock_test(&self.snapshot).clone())
    }
}

struct Harness {
    root: TempDirectory,
    now: Arc<AtomicI64>,
    registry: Arc<Registry>,
    store: Arc<FakeStore>,
    git: Arc<FakeGit>,
    coordinator: Arc<Coordinator>,
}

impl Harness {
    fn new() -> Self {
        let root = TempDirectory::new("ptrack-agent-coordination");
        let canonical = std::fs::canonicalize(root.path()).unwrap();
        let root_string = canonical.to_string_lossy().into_owned();
        let now = Arc::new(AtomicI64::new(1_000));
        let registry_random = Arc::new(AtomicU64::new(1));
        let registry = Arc::new(Registry::new(RegistryConfig {
            project_root: canonical.clone(),
            now: Some({
                let now = Arc::clone(&now);
                Arc::new(move || Timestamp::from_unix_seconds(now.load(Ordering::SeqCst)))
            }),
            random: Some(Arc::new(move |bytes| {
                bytes.fill(0);
                bytes[..8]
                    .copy_from_slice(&registry_random.fetch_add(1, Ordering::SeqCst).to_le_bytes());
                Ok(())
            })),
            additional_cwd_validator: Some(Arc::new(|_| true)),
            ..RegistryConfig::default()
        }));
        let store = Arc::new(FakeStore {
            root: root_string,
            current: AtomicBool::new(true),
            linked: Mutex::new(Vec::new()),
            tracking: AtomicI64::new(0),
            plans: Mutex::new(BTreeMap::from([(7, "Plan title".to_owned())])),
            tasks: Mutex::new(BTreeMap::from([(
                9,
                CoordinationTaskContext {
                    id: 9,
                    plan_id: 7,
                    title: "Task title".to_owned(),
                },
            )])),
            decisions: Mutex::new(Vec::new()),
            issues: Mutex::new(Vec::new()),
        });
        let git = Arc::new(FakeGit::new(&canonical));
        let coordinator_random = Arc::new(AtomicU8::new(100));
        let coordinator = Arc::new(Coordinator::new(CoordinationConfig {
            generation: GENERATION,
            project_root: canonical,
            registry: Arc::clone(&registry),
            store: Arc::clone(&store) as Arc<dyn CoordinationStore>,
            git: Arc::clone(&git) as Arc<dyn CoordinationGit>,
            sessions: Arc::new(FakeSessions::default()),
            now: Some({
                let now = Arc::clone(&now);
                Arc::new(move || Timestamp::from_unix_seconds(now.load(Ordering::SeqCst)))
            }),
            random: Some(Arc::new(move |bytes| {
                bytes.fill(coordinator_random.fetch_add(1, Ordering::SeqCst));
                Ok(())
            })),
            mutation_revision: None,
            runtime_changed: None,
        }));
        Self {
            root,
            now,
            registry,
            store,
            git,
            coordinator,
        }
    }

    fn launched(&self, terminal: &str, plan_id: u64, task_id: u64) -> Run {
        self.launched_at(terminal, plan_id, task_id, self.root.path())
    }

    fn launched_at(&self, terminal: &str, plan_id: u64, task_id: u64, cwd: &Path) -> Run {
        let catalog = Catalog;
        let host = AssociationHost::new(self.root.path(), GENERATION, Some(&catalog)).unwrap();
        self.registry
            .register_linked_launched(
                Registration {
                    profile: "codex".to_owned(),
                    provider: "codex".to_owned(),
                    pid: 42,
                    terminal_id: terminal.to_owned(),
                    cwd: cwd.to_string_lossy().into_owned(),
                },
                Some(&host),
                AssociationPointer {
                    version: ASSOCIATION_VERSION_V1,
                    plan_id,
                    task_id,
                },
            )
            .unwrap()
    }

    fn external(&self) -> Lease {
        self.registry
            .register_external(Registration {
                profile: "external".to_owned(),
                provider: "codex".to_owned(),
                pid: 0,
                terminal_id: String::new(),
                cwd: String::new(),
            })
            .unwrap()
    }
}

#[test]
fn dto_json_is_content_free_and_matches_camel_case_contract() {
    let mutation = AgentWorktreeMutationV2 {
        generation: 7,
        run_id: "run-1".to_owned(),
        associated: true,
        worktree: Some(AgentWorktreeAssociation {
            identity: WorktreeIdentity {
                root: "/project".to_owned(),
                git_dir: "SECRET-GIT-DIR".to_owned(),
                common_git_dir: "SECRET-COMMON-DIR".to_owned(),
                branch: "feature".to_owned(),
                head: "a".repeat(40),
                linked: true,
            },
            verified: true,
            isolated: true,
            cwd_matches: true,
        }),
    };
    assert_eq!(
        serde_json::to_value(&mutation).unwrap(),
        serde_json::json!({
            "generation": 7,
            "runId": "run-1",
            "associated": true,
            "worktree": {
                "identity": {
                    "root": "/project",
                    "branch": "feature",
                    "head": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "linked": true
                },
                "verified": true,
                "isolated": true,
                "cwdMatches": true
            }
        })
    );
    let encoded = serde_json::to_string(&mutation).unwrap();
    for secret in ["SECRET-GIT-DIR", "SECRET-COMMON-DIR", "leaseToken"] {
        assert!(!encoded.contains(secret));
    }
}

#[test]
fn standalone_preview_and_intelligence_are_bounded_current_and_read_only() {
    let harness = Harness::new();
    let lease = harness.external();
    let catalog = Catalog;
    let host = AssociationHost::new(harness.root.path(), GENERATION, Some(&catalog)).unwrap();
    harness
        .registry
        .associate(
            &lease.run.id,
            Some(&host),
            AssociationPointer {
                version: ASSOCIATION_VERSION_V1,
                plan_id: 7,
                task_id: 9,
            },
        )
        .unwrap();
    for sequence in 1..=9 {
        harness
            .registry
            .record_provider_event(
                &lease.run.id,
                &lease.lease_token,
                ProviderEvent {
                    model_version: PROVIDER_EVENT_MODEL_VERSION,
                    id: format!("file-{sequence}"),
                    sequence,
                    event_type: "file.completed".to_owned(),
                    paths: vec![format!("src/file-{sequence}.rs")],
                    ..ProviderEvent::default()
                },
            )
            .unwrap();
    }
    *lock_test(&harness.store.decisions) = (1..=5)
        .map(|id| CoordinationDecision {
            id,
            target: CoordinationTarget::Task,
            target_id: 9,
            body: format!("decision {id}"),
        })
        .collect();
    *lock_test(&harness.store.issues) = (1..=5)
        .map(|id| CoordinationIssue {
            id,
            title: format!("issue {id}"),
            task_id: 9,
        })
        .collect();
    let before = harness.registry.run(&lease.run.id).unwrap();
    let preview = harness
        .coordinator
        .preview_handoff(GENERATION, &lease.run.id)
        .unwrap();
    assert_eq!(preview.event_bounds, BoundedSnapshot::new(9, 9));
    assert_eq!(preview.preview.included_event_ids.len(), 8);
    assert!(preview.preview.truncated);
    let intelligence = harness
        .coordinator
        .agent_intelligence(GENERATION, &lease.run.id)
        .unwrap();
    assert_eq!(intelligence.event_bounds, BoundedSnapshot::new(9, 9));
    assert_eq!(intelligence.bounds, BoundedSnapshot::new(16, 18));
    assert_eq!(intelligence.suggestions[0].label, "Task #9 · Task title");
    assert_eq!(intelligence.suggestions[1].label, "Plan #7 · Plan title");
    assert_eq!(
        intelligence
            .suggestions
            .iter()
            .filter(|item| item.kind == AgentSuggestionKind::File)
            .count(),
        8
    );
    assert_eq!(harness.registry.run(&lease.run.id).unwrap(), before);
    let json = serde_json::to_string(&intelligence).unwrap();
    for forbidden in [&lease.lease_token, "provider", "projectRoot", "summary"] {
        assert!(!json.contains(forbidden));
    }

    harness.store.current.store(false, Ordering::SeqCst);
    let stale = harness
        .coordinator
        .agent_intelligence(GENERATION, &lease.run.id)
        .unwrap();
    assert!(stale.association.is_none());
    assert_eq!(stale.event_bounds, BoundedSnapshot::new(0, 9));
    assert!(stale.suggestions.is_empty());
}

#[test]
fn suggestion_text_cap_repairs_utf8_and_appends_ellipsis() {
    let value = format!("{}é secret", "a".repeat(319));
    let bounded = bounded_suggestion_text(&value);
    assert!(bounded.is_char_boundary(bounded.len()));
    assert!(bounded.ends_with('…'));
    assert!(!bounded.contains("secret"));
    assert!(bounded.len() <= SUGGESTION_TEXT_BYTES + '…'.len_utf8());
}

#[test]
fn projections_ownership_worktree_conflict_and_notifications_are_bounded() {
    let harness = Harness::new();
    let first = harness.launched("terminal-1", 7, 9);
    let second = harness.launched("terminal-2", 7, 9);
    harness
        .coordinator
        .set_task_ownership(GENERATION, &first.id, 1, true)
        .unwrap();
    harness
        .coordinator
        .set_worktree(GENERATION, &first.id, 1, &first.cwd, true)
        .unwrap();
    let snapshot = harness.coordinator.activity(GENERATION).unwrap();
    assert_eq!(snapshot.bounds, BoundedSnapshot::new(2, 2));
    assert_eq!(snapshot.counts.running, 2);
    assert_eq!(snapshot.conflicts.len(), 1);
    assert_eq!(snapshot.conflicts[0].owner_count, 1);
    let first_item = snapshot
        .items
        .iter()
        .find(|item| item.run_id == first.id)
        .unwrap();
    assert!(first_item.ownership.is_some());
    assert!(
        first_item
            .worktree
            .as_ref()
            .is_some_and(|value| value.verified)
    );
    assert_eq!(snapshot.worktree_bounds, BoundedSnapshot::new(1, 1));
    assert_eq!(snapshot.workflow_targets, vec!["main"]);
    assert_eq!(harness.git.inspections.load(Ordering::SeqCst), 1);

    let json = serde_json::to_string(&snapshot).unwrap();
    for secret in ["leaseToken", "SECRET", "environment", "prompt", "title"] {
        assert!(!json.contains(secret));
    }
    assert_eq!(second.association.unwrap().target.task_id, 9);
}

#[test]
fn exact_projection_and_notification_boundaries_mark_only_real_omissions() {
    let harness = Harness::new();
    for _ in 0..65 {
        harness.external();
    }
    let runs = harness.coordinator.agent_runs(GENERATION).unwrap();
    assert_eq!(runs.runs.len(), 64);
    assert_eq!(runs.bounds, BoundedSnapshot::new(64, 65));
    assert!(
        harness
            .coordinator
            .activity(GENERATION)
            .unwrap()
            .analysis_incomplete
    );

    let notification_harness = Harness::new();
    let lease = notification_harness.external();
    for sequence in 1..=33 {
        notification_harness
            .registry
            .record_provider_event(
                &lease.run.id,
                &lease.lease_token,
                ProviderEvent {
                    model_version: PROVIDER_EVENT_MODEL_VERSION,
                    id: format!("permission-{sequence}"),
                    sequence,
                    event_type: "permissionrequest".to_owned(),
                    ..ProviderEvent::default()
                },
            )
            .unwrap();
    }
    let activity = notification_harness
        .coordinator
        .activity(GENERATION)
        .unwrap();
    assert_eq!(activity.notifications.len(), 1);
    assert_eq!(activity.notification_bounds, BoundedSnapshot::new(1, 1));
    assert!(activity.notifications_incomplete);
}

#[test]
fn exact_snapshot_accepts_the_full_1024_record_ceiling() {
    let harness = Harness::new();
    for _ in 0..CANDIDATE_LIMIT {
        harness.external();
    }
    let runs = harness.coordinator.agent_runs(GENERATION).unwrap();
    assert_eq!(runs.bounds, BoundedSnapshot::new(64, CANDIDATE_LIMIT));
}

#[test]
fn conflict_projection_caps_targets_and_sorted_run_ids_exactly() {
    let mut runs = Vec::new();
    for task_id in 1..=65 {
        for suffix in ["b", "a"] {
            runs.push(AgentRuntimeSummary {
                run_id: format!("{task_id:03}-{suffix}"),
                registration_kind: RegistrationKind::Launched,
                terminal_id: String::new(),
                terminal_backed: false,
                terminal_present: false,
                corresponding_terminal: false,
                state: RunState::Running,
                process_state: ProcessState::Running,
                lease_state: LeaseState::None,
                live: true,
                activity_state: ActivityState::Running,
                association: Some(RuntimeAssociation {
                    plan_id: 1,
                    task_id,
                    revision: 1,
                }),
                intelligence: None,
            });
        }
    }
    let (conflicts, bounds) = activity_conflicts(&runs, &BTreeMap::new());
    assert_eq!(conflicts.len(), 64);
    assert_eq!(bounds, BoundedSnapshot::new(64, 65));
    assert_eq!(conflicts[0].run_ids, vec!["001-a", "001-b"]);
}

#[test]
fn handoff_acknowledgement_is_exactly_once_under_concurrency() {
    let harness = Harness::new();
    let source = harness.launched("source", 7, 9);
    let target = harness.launched("target", 7, 10);
    let handoff = harness
        .coordinator
        .send_handoff(GENERATION, &source.id, &target.id, 1, 1)
        .unwrap();
    assert!(handoff.preview.text.contains("plan #7, task #9"));
    assert!(!handoff.preview.text.contains("lease"));
    let barrier = Arc::new(Barrier::new(3));
    let mut threads = Vec::new();
    for _ in 0..2 {
        let coordinator = Arc::clone(&harness.coordinator);
        let barrier = Arc::clone(&barrier);
        let id = handoff.id.clone();
        let target = target.id.clone();
        threads.push(thread::spawn(move || {
            barrier.wait();
            coordinator.acknowledge_handoff(GENERATION, &id, &target)
        }));
    }
    barrier.wait();
    let results = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(CoordinationError::HandoffStale)))
            .count(),
        1
    );
}

#[test]
fn stale_handoff_ack_consumes_and_maps_inactive_but_wrong_target_does_not_consume() {
    let harness = Harness::new();
    let source = harness.launched("source-stale", 7, 9);
    let target = harness.launched("target-stale", 7, 10);
    let handoff = harness
        .coordinator
        .send_handoff(GENERATION, &source.id, &target.id, 1, 1)
        .unwrap();
    assert_eq!(
        harness
            .coordinator
            .acknowledge_handoff(GENERATION, &handoff.id, &source.id)
            .unwrap_err(),
        CoordinationError::HandoffStale
    );
    assert_eq!(
        harness
            .coordinator
            .activity(GENERATION)
            .unwrap()
            .handoffs
            .items
            .len(),
        1
    );
    assert!(
        harness
            .registry
            .record_terminal_exit("source-stale", 1, "failed")
    );
    assert_eq!(
        harness
            .coordinator
            .acknowledge_handoff(GENERATION, &handoff.id, &target.id)
            .unwrap_err(),
        CoordinationError::HandoffStale
    );
    assert!(
        harness
            .coordinator
            .activity(GENERATION)
            .unwrap()
            .handoffs
            .items
            .is_empty()
    );
}

#[test]
fn handoff_post_preview_lifecycle_and_association_races_map_stale_without_insert() {
    let lifecycle = Harness::new();
    let source = lifecycle.launched("slow-source", 7, 9);
    let target = lifecycle.launched("slow-target", 7, 10);
    let started = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    lifecycle
        .coordinator
        .install_preview_barrier(Arc::clone(&started), Arc::clone(&release));
    let coordinator = Arc::clone(&lifecycle.coordinator);
    let source_id = source.id.clone();
    let target_id = target.id.clone();
    let send =
        thread::spawn(move || coordinator.send_handoff(GENERATION, &source_id, &target_id, 1, 1));
    started.wait();
    assert!(
        lifecycle
            .registry
            .record_terminal_exit("slow-source", 1, "failed")
    );
    release.wait();
    assert_eq!(
        send.join().unwrap().unwrap_err(),
        CoordinationError::HandoffStale
    );
    assert!(
        lifecycle
            .coordinator
            .activity(GENERATION)
            .unwrap()
            .handoffs
            .items
            .is_empty()
    );

    let association = Harness::new();
    let source = association.launched("assoc-source", 7, 9);
    let target = association.launched("assoc-target", 7, 10);
    let started = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    association
        .coordinator
        .install_preview_barrier(Arc::clone(&started), Arc::clone(&release));
    let coordinator = Arc::clone(&association.coordinator);
    let source_id = source.id.clone();
    let target_id = target.id.clone();
    let send =
        thread::spawn(move || coordinator.send_handoff(GENERATION, &source_id, &target_id, 1, 1));
    started.wait();
    association.store.current.store(false, Ordering::SeqCst);
    release.wait();
    assert_eq!(
        send.join().unwrap().unwrap_err(),
        CoordinationError::HandoffStale
    );
    assert!(
        association
            .coordinator
            .activity(GENERATION)
            .unwrap()
            .handoffs
            .items
            .is_empty()
    );
}

#[test]
fn workflow_approval_is_exactly_once_and_read_only() {
    let harness = Harness::new();
    let run = harness.launched("workflow", 7, 9);
    let proposal = harness
        .coordinator
        .prepare_workflow(
            GENERATION,
            &run.id,
            1,
            AgentWorkflowKind::PullRequest,
            "main",
        )
        .unwrap();
    assert_eq!(proposal.state, AgentWorkflowState::Proposed);
    assert_eq!(proposal.notice, AGENT_WORKFLOW_NOTICE);
    let barrier = Arc::new(Barrier::new(3));
    let mut threads = Vec::new();
    for _ in 0..2 {
        let coordinator = Arc::clone(&harness.coordinator);
        let barrier = Arc::clone(&barrier);
        let id = proposal.id.clone();
        threads.push(thread::spawn(move || {
            barrier.wait();
            coordinator.approve_workflow(GENERATION, &id)
        }));
    }
    barrier.wait();
    let results = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(CoordinationError::WorkflowApproved)))
            .count(),
        1
    );
    assert!(results.iter().flatten().all(|value| {
        value.state == AgentWorkflowState::Approved && value.approved_at.is_some()
    }));
}

#[test]
fn handoff_and_workflow_inboxes_accept_64_then_fail_closed() {
    let handoffs = Harness::new();
    let source = handoffs.launched("source-cap", 7, 9);
    let target = handoffs.launched("target-cap", 7, 10);
    for _ in 0..INBOX_LIMIT {
        handoffs
            .coordinator
            .send_handoff(GENERATION, &source.id, &target.id, 1, 1)
            .unwrap();
    }
    assert_eq!(
        handoffs
            .coordinator
            .send_handoff(GENERATION, &source.id, &target.id, 1, 1)
            .unwrap_err(),
        CoordinationError::HandoffFull
    );
    let inbox = handoffs.coordinator.activity(GENERATION).unwrap().handoffs;
    assert_eq!(inbox.items.len(), INBOX_LIMIT);
    assert_eq!(inbox.bounds, BoundedSnapshot::new(INBOX_LIMIT, INBOX_LIMIT));

    let workflows = Harness::new();
    let run = workflows.launched("workflow-cap", 7, 9);
    for _ in 0..INBOX_LIMIT {
        workflows
            .coordinator
            .prepare_workflow(GENERATION, &run.id, 1, AgentWorkflowKind::Validation, "")
            .unwrap();
    }
    assert_eq!(
        workflows
            .coordinator
            .prepare_workflow(GENERATION, &run.id, 1, AgentWorkflowKind::Validation, "")
            .unwrap_err(),
        CoordinationError::WorkflowFull
    );
    let inbox = workflows
        .coordinator
        .activity(GENERATION)
        .unwrap()
        .workflows;
    assert_eq!(inbox.items.len(), INBOX_LIMIT);
    assert_eq!(inbox.bounds, BoundedSnapshot::new(INBOX_LIMIT, INBOX_LIMIT));
}

#[test]
fn ttl_lifecycle_and_catalog_changes_fail_closed() {
    let harness = Harness::new();
    let source = harness.launched("source", 7, 9);
    let target = harness.launched("target", 7, 10);
    let handoff = harness
        .coordinator
        .send_handoff(GENERATION, &source.id, &target.id, 1, 1)
        .unwrap();
    harness.now.store(1_000 + 30 * 60, Ordering::SeqCst);
    assert_eq!(
        harness
            .coordinator
            .acknowledge_handoff(GENERATION, &handoff.id, &target.id)
            .unwrap_err(),
        CoordinationError::HandoffStale
    );

    harness.store.current.store(false, Ordering::SeqCst);
    assert_eq!(
        harness
            .coordinator
            .set_task_ownership(GENERATION, &source.id, 1, true)
            .unwrap_err(),
        CoordinationError::OwnershipRequiresTask
    );
}

#[test]
fn stale_generation_and_shutdown_clear_all_ephemeral_state() {
    let harness = Harness::new();
    let run = harness.launched("run", 7, 9);
    assert_eq!(
        harness.coordinator.agent_runs(8).unwrap_err(),
        CoordinationError::StaleGeneration {
            expected: 8,
            active: GENERATION
        }
    );
    harness
        .coordinator
        .set_task_ownership(GENERATION, &run.id, 1, true)
        .unwrap();
    harness.coordinator.shutdown();
    assert_eq!(
        harness.coordinator.activity(GENERATION).unwrap_err(),
        CoordinationError::Closed
    );
}

#[test]
fn unsupported_workflow_and_stale_catalog_use_exact_fail_closed_errors() {
    let harness = Harness::new();
    let run = harness.launched("run", 7, 9);
    assert_eq!(
        harness
            .coordinator
            .prepare_workflow(GENERATION, &run.id, 1, AgentWorkflowKind::Unset, "")
            .unwrap_err(),
        CoordinationError::WorkflowKind
    );
    harness.store.current.store(false, Ordering::SeqCst);
    assert_eq!(
        harness
            .coordinator
            .set_task_ownership(GENERATION, &run.id, 1, true)
            .unwrap_err(),
        CoordinationError::OwnershipRequiresTask
    );
    assert_eq!(
        harness
            .coordinator
            .set_worktree(GENERATION, &run.id, 1, &run.cwd, true)
            .unwrap_err(),
        CoordinationError::WorktreeRevision
    );
}

#[test]
fn workflow_binding_mutation_is_revalidated_and_proposal_is_removed() {
    let harness = Harness::new();
    let run = harness.launched("workflow", 7, 9);
    let proposal = harness
        .coordinator
        .prepare_workflow(GENERATION, &run.id, 1, AgentWorkflowKind::Validation, "")
        .unwrap();
    lock_test(&harness.git.snapshot).head = "c".repeat(40);
    assert_eq!(
        harness
            .coordinator
            .approve_workflow(GENERATION, &proposal.id)
            .unwrap_err(),
        CoordinationError::WorkflowStale
    );
    assert_eq!(
        harness
            .coordinator
            .approve_workflow(GENERATION, &proposal.id)
            .unwrap_err(),
        CoordinationError::WorkflowStale
    );
}

#[test]
fn workflow_approval_consumes_and_maps_every_recapture_failure_to_stale() {
    let exit = Harness::new();
    let run = exit.launched("approve-exit", 7, 9);
    let proposal = exit
        .coordinator
        .prepare_workflow(GENERATION, &run.id, 1, AgentWorkflowKind::Validation, "")
        .unwrap();
    assert!(
        exit.registry
            .record_terminal_exit("approve-exit", 1, "failed")
    );
    assert_workflow_stale_and_consumed(&exit.coordinator, &proposal.id);

    let catalog = Harness::new();
    let run = catalog.launched("approve-catalog", 7, 9);
    let proposal = catalog
        .coordinator
        .prepare_workflow(GENERATION, &run.id, 1, AgentWorkflowKind::Validation, "")
        .unwrap();
    catalog.store.current.store(false, Ordering::SeqCst);
    assert_workflow_stale_and_consumed(&catalog.coordinator, &proposal.id);

    let reassociated = Harness::new();
    let lease = reassociated.external();
    let association_catalog = Catalog;
    let host = AssociationHost::new(
        reassociated.root.path(),
        GENERATION,
        Some(&association_catalog),
    )
    .unwrap();
    reassociated
        .registry
        .associate(
            &lease.run.id,
            Some(&host),
            AssociationPointer {
                version: ASSOCIATION_VERSION_V1,
                plan_id: 7,
                task_id: 9,
            },
        )
        .unwrap();
    let proposal = reassociated
        .coordinator
        .prepare_workflow(
            GENERATION,
            &lease.run.id,
            1,
            AgentWorkflowKind::Validation,
            "",
        )
        .unwrap();
    reassociated
        .registry
        .associate(
            &lease.run.id,
            Some(&host),
            AssociationPointer {
                version: ASSOCIATION_VERSION_V1,
                plan_id: 7,
                task_id: 10,
            },
        )
        .unwrap();
    assert_workflow_stale_and_consumed(&reassociated.coordinator, &proposal.id);

    let target = Harness::new();
    let run = target.launched("approve-target", 7, 9);
    let proposal = target
        .coordinator
        .prepare_workflow(
            GENERATION,
            &run.id,
            1,
            AgentWorkflowKind::PullRequest,
            "main",
        )
        .unwrap();
    lock_test(&target.git.snapshot).branches.clear();
    assert_workflow_stale_and_consumed(&target.coordinator, &proposal.id);

    let git_error = Harness::new();
    let run = git_error.launched("approve-git-error", 7, 9);
    let proposal = git_error
        .coordinator
        .prepare_workflow(GENERATION, &run.id, 1, AgentWorkflowKind::Validation, "")
        .unwrap();
    git_error.git.snapshot_error.store(true, Ordering::SeqCst);
    assert_workflow_stale_and_consumed(&git_error.coordinator, &proposal.id);
}

#[test]
fn workflow_digest_revalidates_every_extended_git_field_but_sorts_paths() {
    let harness = Harness::new();
    let run = harness.launched("workflow-digest", 7, 9);
    for field in [
        "upstream",
        "detached",
        "initial",
        "changedMore",
        "untrackedMore",
        "divergence",
    ] {
        let proposal = harness
            .coordinator
            .prepare_workflow(GENERATION, &run.id, 1, AgentWorkflowKind::Validation, "")
            .unwrap();
        let original = lock_test(&harness.git.snapshot).clone();
        {
            let mut snapshot = lock_test(&harness.git.snapshot);
            match field {
                "upstream" => snapshot.upstream = "origin/main".to_owned(),
                "detached" => snapshot.detached = true,
                "initial" => snapshot.initial = true,
                "changedMore" => snapshot.changed_more = 1,
                "untrackedMore" => snapshot.untracked_more = 1,
                "divergence" => {
                    snapshot.divergence = Some(GitDivergence {
                        upstream: "origin/main".to_owned(),
                        ahead: 1,
                        behind: 2,
                    });
                }
                _ => unreachable!(),
            }
        }
        assert_eq!(
            harness
                .coordinator
                .approve_workflow(GENERATION, &proposal.id)
                .unwrap_err(),
            CoordinationError::WorkflowStale,
            "{field}"
        );
        *lock_test(&harness.git.snapshot) = original;
    }
    {
        let mut snapshot = lock_test(&harness.git.snapshot);
        snapshot.changed_paths = vec!["b".to_owned(), "a".to_owned()];
        snapshot.untracked_paths = vec!["d".to_owned(), "c".to_owned()];
    }
    let proposal = harness
        .coordinator
        .prepare_workflow(GENERATION, &run.id, 1, AgentWorkflowKind::Validation, "")
        .unwrap();
    {
        let mut snapshot = lock_test(&harness.git.snapshot);
        snapshot.changed_paths.reverse();
        snapshot.untracked_paths.reverse();
    }
    assert!(
        harness
            .coordinator
            .approve_workflow(GENERATION, &proposal.id)
            .is_ok()
    );
}

#[test]
fn workflow_prepare_rejects_claim_detach_during_slow_git_without_locking_host_calls() {
    let harness = Harness::new();
    let run = harness.launched("workflow-race", 7, 9);
    harness
        .coordinator
        .set_worktree(GENERATION, &run.id, 1, &run.cwd, true)
        .unwrap();
    let started = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    *lock_test(&harness.git.inspect_block) = Some((Arc::clone(&started), Arc::clone(&release)));
    let coordinator = Arc::clone(&harness.coordinator);
    let run_id = run.id.clone();
    let prepare = thread::spawn(move || {
        coordinator.prepare_workflow(GENERATION, &run_id, 1, AgentWorkflowKind::Validation, "")
    });
    started.wait();
    harness
        .coordinator
        .set_worktree(GENERATION, &run.id, 1, "", false)
        .unwrap();
    release.wait();
    *lock_test(&harness.git.inspect_block) = None;
    assert_eq!(
        prepare.join().unwrap().unwrap_err(),
        CoordinationError::WorkflowStale
    );
    assert!(
        harness
            .coordinator
            .activity(GENERATION)
            .unwrap()
            .workflows
            .items
            .is_empty()
    );
}

#[test]
fn workflow_prepare_rejects_claim_change_during_slow_snapshot() {
    let harness = Harness::new();
    let run = harness.launched("workflow-snapshot-race", 7, 9);
    harness
        .coordinator
        .set_worktree(GENERATION, &run.id, 1, &run.cwd, true)
        .unwrap();
    let started = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    *lock_test(&harness.git.snapshot_block) = Some((Arc::clone(&started), Arc::clone(&release)));
    let coordinator = Arc::clone(&harness.coordinator);
    let run_id = run.id.clone();
    let prepare = thread::spawn(move || {
        coordinator.prepare_workflow(GENERATION, &run_id, 1, AgentWorkflowKind::Validation, "")
    });
    started.wait();
    harness
        .coordinator
        .set_worktree(GENERATION, &run.id, 1, "", false)
        .unwrap();
    release.wait();
    *lock_test(&harness.git.snapshot_block) = None;
    assert_eq!(
        prepare.join().unwrap().unwrap_err(),
        CoordinationError::WorkflowStale
    );
}

#[test]
fn workflow_inbox_prunes_both_worktree_attach_and_detach_epoch_changes() {
    let detach = Harness::new();
    let run = detach.launched("detach", 7, 9);
    detach
        .coordinator
        .set_worktree(GENERATION, &run.id, 1, &run.cwd, true)
        .unwrap();
    detach
        .coordinator
        .prepare_workflow(GENERATION, &run.id, 1, AgentWorkflowKind::Validation, "")
        .unwrap();
    detach
        .coordinator
        .set_worktree(GENERATION, &run.id, 1, "", false)
        .unwrap();
    assert!(
        detach
            .coordinator
            .activity(GENERATION)
            .unwrap()
            .workflows
            .items
            .is_empty()
    );

    let attach = Harness::new();
    let run = attach.launched("attach", 7, 9);
    attach
        .coordinator
        .prepare_workflow(GENERATION, &run.id, 1, AgentWorkflowKind::Validation, "")
        .unwrap();
    attach
        .coordinator
        .set_worktree(GENERATION, &run.id, 1, &run.cwd, true)
        .unwrap();
    assert!(
        attach
            .coordinator
            .activity(GENERATION)
            .unwrap()
            .workflows
            .items
            .is_empty()
    );
}

#[test]
fn workflow_requires_explicit_verified_claim_for_outside_project_cwd() {
    let harness = Harness::new();
    let outside = TempDirectory::new("ptrack-agent-coordination-sibling");
    let run = harness.launched_at("outside", 7, 9, outside.path());
    let snapshots_before = harness.git.snapshots.load(Ordering::SeqCst);
    assert_eq!(
        harness
            .coordinator
            .prepare_workflow(GENERATION, &run.id, 1, AgentWorkflowKind::Validation, "")
            .unwrap_err(),
        CoordinationError::WorkflowStale
    );
    assert_eq!(
        harness.git.snapshots.load(Ordering::SeqCst),
        snapshots_before
    );

    let outside_root = std::fs::canonicalize(outside.path())
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let identity = WorktreeIdentity {
        root: outside_root.clone(),
        git_dir: format!("{outside_root}/.git-worktree"),
        common_git_dir: lock_test(&harness.git.identity).common_git_dir.clone(),
        branch: "sibling".to_owned(),
        head: "e".repeat(40),
        linked: true,
    };
    *lock_test(&harness.git.identity) = identity.clone();
    {
        let mut snapshot = lock_test(&harness.git.snapshot);
        snapshot.root = identity.root.clone();
        snapshot.git_dir = identity.git_dir.clone();
        snapshot.common_git_dir = identity.common_git_dir.clone();
        snapshot.branch = identity.branch.clone();
        snapshot.head = identity.head.clone();
    }
    harness
        .coordinator
        .set_worktree(GENERATION, &run.id, 1, &outside_root, true)
        .unwrap();
    assert!(
        harness
            .coordinator
            .prepare_workflow(GENERATION, &run.id, 1, AgentWorkflowKind::Validation, "")
            .is_ok()
    );
}

#[test]
fn drift_is_sorted_bounded_and_does_not_reveal_unstructured_run_content() {
    let harness = Harness::new();
    let run = harness.launched("run", 7, 9);
    harness
        .coordinator
        .set_task_ownership(GENERATION, &run.id, 1, true)
        .unwrap();
    {
        let mut git = lock_test(&harness.git.snapshot);
        git.changed_paths = vec!["src/lib.rs".to_owned()];
        git.untracked_paths = vec!["private.tmp".to_owned()];
        git.recent_commits = vec![GitCommit {
            sha: "d".repeat(40),
            committed_at: Timestamp::from_unix_seconds(1_000),
        }];
    }
    let drift = harness.coordinator.drift(GENERATION).unwrap();
    assert_eq!(drift.state, "ready");
    assert_eq!(drift.findings[0].severity, "warning");
    assert!(
        drift
            .findings
            .iter()
            .any(|item| item.kind == "checkoutChangedPath")
    );
    assert!(
        drift
            .findings
            .iter()
            .any(|item| item.kind == "unlinkedCommit")
    );
    let json = serde_json::to_string(&drift).unwrap();
    assert!(!json.contains("lease"));
    assert!(!json.contains("result"));
}

#[test]
fn drift_uses_unpushed_recent_cutoff_and_only_unique_commit_prefixes() {
    let harness = Harness::new();
    harness.store.tracking.store(1_000, Ordering::SeqCst);
    let equal = format!("1111111{}", "1".repeat(33));
    let unlinked = format!("2222222{}", "2".repeat(33));
    let ambiguous_a = format!("abcdef0{}", "0".repeat(33));
    let ambiguous_b = format!("abcdef0{}", "1".repeat(33));
    let exact = "f".repeat(40);
    *lock_test(&harness.store.linked) =
        vec!["1111111".to_owned(), "abcdef0".to_owned(), exact.clone()];
    {
        let mut git = lock_test(&harness.git.snapshot);
        git.unpushed_commits = vec![
            GitCommit {
                sha: "0".repeat(40),
                committed_at: Timestamp::from_unix_seconds(999),
            },
            GitCommit {
                sha: equal.clone(),
                committed_at: Timestamp::from_unix_seconds(1_000),
            },
            GitCommit {
                sha: unlinked.clone(),
                committed_at: Timestamp::from_unix_seconds(1_001),
            },
        ];
        git.recent_commits = vec![
            GitCommit {
                sha: unlinked.clone(),
                committed_at: Timestamp::from_unix_seconds(1_001),
            },
            GitCommit {
                sha: ambiguous_a.clone(),
                committed_at: Timestamp::from_unix_seconds(1_002),
            },
            GitCommit {
                sha: ambiguous_b.clone(),
                committed_at: Timestamp::from_unix_seconds(1_002),
            },
            GitCommit {
                sha: exact,
                committed_at: Timestamp::from_unix_seconds(1_002),
            },
            GitCommit {
                sha: "d".repeat(40),
                committed_at: Timestamp::ZERO,
            },
        ];
        git.recent_commits_incomplete = true;
    }
    let drift = harness.coordinator.drift(GENERATION).unwrap();
    let shas = drift
        .findings
        .iter()
        .filter(|finding| finding.kind == "unlinkedCommit")
        .map(|finding| finding.sha.clone())
        .collect::<BTreeSet<_>>();
    assert_eq!(shas, BTreeSet::from([unlinked, ambiguous_a, ambiguous_b]));
    assert!(drift.incomplete);
}

fn lock_test<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn assert_workflow_stale_and_consumed(coordinator: &Coordinator, id: &str) {
    assert_eq!(
        coordinator.approve_workflow(GENERATION, id).unwrap_err(),
        CoordinationError::WorkflowStale
    );
    assert_eq!(
        coordinator.approve_workflow(GENERATION, id).unwrap_err(),
        CoordinationError::WorkflowStale
    );
}

#[allow(dead_code)]
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap()
}
