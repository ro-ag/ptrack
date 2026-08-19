# Plan Lifecycle Operations Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the approved plan-lifecycle design (`docs/superpowers/specs/2026-08-19-plan-lifecycle-ops-design.md`): `ptrack plan delete <id> --force` (guarded hard delete with a full cascade), `ptrack plan move <id> --to <project> [--as <title>]` (copy-into-target-first, then delete-from-source), `ptrack plan copy <id> [--to <project>] [--as <title>]` (subtree duplication with reminted IDs), and the Desktop GUI's first plan-level content mutations: Rename, Delete (with cascade preview), Move, Copy (with a project picker), backed by five new desktop bridge commands. No payload-schema change of any kind — these are operations over existing record shapes.

**Architecture:** ptrack is a Rust workspace. `ptrack-store` owns redb storage (`ProjectStore`/`GlobalStore`); the delete cascade and the subtree export/import engine land there so every surface is correct by construction. `ptrack-app` owns the `ApplicationPort` seam — a new `plan_lifecycle` port method (mirroring the `mutate(Mutation)` precedent) carries all three operations, and `LocalApplication` resolves cross-project targets through the global registry plus `ActiveRuntime::bindings_for_exact_root` (the cutover lock is a shared per-host lease, so opening two `ProjectStore`s in one process is legal — `production_test.rs:690` already initializes two projects under one home). `ptrack-cli` registers the three commands in its five parallel registration lists. The desktop runtime (`desktop_runtime.rs`) gains five allowlisted commands (`RenamePlanV1`, `DeletePlanV1`, `MovePlanV1`, `CopyPlanV1`, `ListProjectsV1`); the vanilla-JS frontend gains a plan context menu, inline rename, a delete-confirmation dialog fed by the preview call, and a move/copy dialog with a project dropdown.

**Tech Stack:** Rust (redb, clap, serde_json — all existing workspace dependencies; no new dependencies), vanilla JS/TS frontend (vitest), Python help-site checker (`tools/help_check.py`).

## Global Constraints

- Work on branch `plan-lifecycle-ops` (create it from `main` before Task 1 if it does not exist: `git checkout -b plan-lifecycle-ops`). Never commit to `main`.
- NO payload-schema changes: `NATIVE_PAYLOAD_SCHEMA` stays 3, `MIN_NATIVE_PAYLOAD_SCHEMA` stays 1, no new fields on `Plan`/`Task`/`Note`/`Issue`/`Commit`/`Meta`, no codec edits. Refuse any design step that adds a record field.
- New CLI commands must be registered in ALL FIVE registration lists (missing one breaks preflight, arg validation, or help): (1) clap tree `crates/ptrack-cli/src/tree.rs`; (2) `LEAVES` in `crates/ptrack-cli/src/command.rs`; (3) `GROUPS` in `crates/ptrack-cli/src/parse.rs`; (4) `flag_names` in `crates/ptrack-cli/src/parse.rs`; (5) help tables in `crates/ptrack-cli/src/help.rs` (`PLAN_CHILDREN` + `plan_leaf` + `group_children`).
- Every new `ApplicationPort` method must be implemented on ALL FIVE impls in the same commit: `UnavailableApplication` (service.rs), `LocalApplication` (service.rs), `RoutedApplication` (production.rs), `FakeApplication` in `crates/ptrack-cli/src/dispatch_test.rs`, `FakeApplication` in `crates/ptrack-tui/src/runtime_test.rs`.
- Rust unit tests go in Go-style sibling files (`module.rs` + `module_test.rs`); NEVER add `#[cfg(test)] mod tests` blocks inside source files.
- Quality gates that must ALL pass before EVERY commit: `cargo test --workspace --all-targets --no-fail-fast`; `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` (this rustdoc gate broke a release once — no intra-doc links to private items in public doc comments); `npm --prefix frontend test` (run `npm --prefix frontend ci` once first); `make help-check` after any change to `docs/help/`, `CHANGELOG.md`, or the UI-source set (`frontend/index.html`, `frontend/src/app.js`, `frontend/src/style.css`, `frontend/src/theme.js`, `frontend/src/workspace/model.ts`, `frontend/src/workspace/presentation.ts`) — the screenshot manifest pins `uiSourceSha256` over those files (Task 8 refreshes it).
- Known env flake: the ptrack-capability 407-metadata test can fail spuriously in this environment — re-run it alone before concluding a regression.
- Conventional-commit messages; NEVER add `Co-Authored-By` or any AI attribution to commits or PRs.
- Keep the workspace green at every commit.

---

### Task 1: Store cascade delete with preview summary

**Files:**
- Modify: `crates/ptrack-store/src/project.rs`
- Modify: `crates/ptrack-store/src/lib.rs` (export)
- Test: `crates/ptrack-store/src/project_test.rs`

**Interfaces:**
- Consumes (all already in `project.rs`): `self.read(...)` / `self.write(...)` funnels, `required_write::<R>(transaction, RecordKey::Id(id))`, `require_claim_access(&WriteTransaction, &Plan, Option<&str>)`, `typed::scan::<R>(&ReadTransaction)`, `typed::scan_write::<R>(&WriteTransaction)`, `typed::put`, `transaction.delete(Collection, RecordKey)`, `stamp_meta(&mut Meta, Timestamp, String)`, `self.clock.now_local()`, `self.actor_id()`.
- Produces:
  - `pub struct PlanDeleteSummary { pub plan_id: u64, pub title: String, pub tasks: usize, pub notes: usize, pub commits_unlinked: usize, pub detached_issues: Vec<(u64, String)> }` (derives `Clone, Debug, Eq, PartialEq`)
  - `pub fn ProjectStore::plan_delete_preview(&self, plan_id: u64) -> StoreResult<PlanDeleteSummary>` (read-only, ungated)
  - `pub fn ProjectStore::delete_plan(&self, plan_id: u64) -> StoreResult<PlanDeleteSummary>` (claim-gated, one write transaction; DETACHES linked issues)
  - `pub fn ProjectStore::delete_plan_for_move(&self, plan_id: u64) -> StoreResult<PlanDeleteSummary>` (same cascade, but DELETES linked issues — a moved task's issue moves with it to the target, so leaving a detached duplicate behind would violate the spec's "issues follow their task"; both public methods share one private `delete_plan_inner(plan_id, detach_issues: bool)`)
  - private `fn cascade_summary(plan: &Plan, tasks: &[Task], notes: &[Note], issues: &[Issue], commits: &[Commit]) -> PlanDeleteSummary`
- Milestone unlink is implicit and total: a `Milestone` record holds no plan list — membership IS `plan.milestone_id` — so deleting the plan record removes it from its milestone with no further write.
- Commit records are never destroyed by either variant: they survive in the source with plan/task references zeroed (the audit trail outlives the plan), exactly like `convert_task_to_plan` treats them.

**Steps:**

- [ ] `git checkout -b plan-lifecycle-ops` (skip if the branch already exists and is checked out).
- [ ] Write failing tests in `crates/ptrack-store/src/project_test.rs` (reuse the file's existing `Temp`, `binding`, `clock` helpers; add `PlanDeleteSummary` to the `use crate::{...}` import):

```rust
#[test]
fn delete_plan_cascades_tasks_notes_detaches_issues_and_zeroes_commits() {
    let temp = Temp::new();
    let store = ProjectStore::create_new_with_clock(
        temp.path("delete.redb"),
        binding(&temp.path("delete.redb"), StoreKind::Project, "delete-1"),
        "test",
        clock(),
    )
    .unwrap();
    let doomed = store.add_plan("Doomed", 0).unwrap();
    let survivor = store.add_plan("Survivor", 0).unwrap();
    let task = store.add_task(doomed.id, "dead task").unwrap();
    let kept_task = store.add_task(survivor.id, "kept task").unwrap();
    store
        .add_note(NoteTarget::Plan, doomed.id, "plan note")
        .unwrap();
    store
        .add_note(NoteTarget::Task, task.id, "task note")
        .unwrap();
    store
        .add_note(NoteTarget::Task, kept_task.id, "kept note")
        .unwrap();
    let issue = store
        .add_issue("crash on save", "", None, task.id)
        .unwrap();
    store.add_commit("aaa111", "linked", doomed.id, task.id).unwrap();
    store.add_commit("bbb222", "kept", survivor.id, 0).unwrap();
    store.set_active_plan(doomed.id).unwrap();

    let summary = store.delete_plan(doomed.id).unwrap();
    assert_eq!(summary.plan_id, doomed.id);
    assert_eq!(summary.title, "Doomed");
    assert_eq!(summary.tasks, 1);
    assert_eq!(summary.notes, 2);
    assert_eq!(summary.commits_unlinked, 1);
    assert_eq!(summary.detached_issues, vec![(issue.id, "crash on save".to_owned())]);

    // Sweep every collection: nothing references the dead plan or its task.
    let snapshot = store.snapshot().unwrap();
    assert!(snapshot.plans.iter().all(|plan| plan.id != doomed.id));
    assert!(snapshot.tasks.iter().all(|task_| task_.plan_id != doomed.id));
    assert!(
        snapshot
            .notes
            .iter()
            .all(|note| !(note.target == NoteTarget::Task && note.target_id == task.id)
                && !(note.target == NoteTarget::Plan && note.target_id == doomed.id))
    );
    let detached = snapshot.issues.iter().find(|i| i.id == issue.id).unwrap();
    assert_eq!(detached.task_id, 0);
    let unlinked = snapshot
        .commits
        .iter()
        .find(|commit| commit.sha == "aaa111")
        .unwrap();
    assert_eq!((unlinked.plan_id, unlinked.task_id), (0, 0));
    let kept = snapshot
        .commits
        .iter()
        .find(|commit| commit.sha == "bbb222")
        .unwrap();
    assert_eq!(kept.plan_id, survivor.id);
    // Legacy active-plan singleton reset to 0.
    assert_eq!(store.meta().unwrap().active_plan, 0);
}

#[test]
fn delete_plan_resets_every_actor_pointer_and_respects_claims() {
    let temp = Temp::new();
    let path = temp.path("delete-claims.redb");
    let expected = binding(&path, StoreKind::Project, "delete-claims-1");
    let alice = ProjectStore::create_new_with_clock(&path, expected.clone(), "test", clock())
        .unwrap()
        .with_actor(Some(ActorIdentity {
            id: "01hzvyekq3s7m8w9x0aaaaaaaa".to_owned(),
            name: "Alice".to_owned(),
        }));
    let bob = ProjectStore::open_existing(&path, &expected, "test")
        .unwrap()
        .with_actor(Some(ActorIdentity {
            id: "01hzvyekq3s7m8w9x0bbbbbbbb".to_owned(),
            name: "Bob".to_owned(),
        }));
    let plan = alice.add_plan("Claimed", 0).unwrap();
    alice.use_plan(plan.id, false).unwrap();
    bob.use_plan(plan.id, true).unwrap(); // Bob steals and points at it too.
    alice.use_plan(plan.id, true).unwrap(); // Alice steals back; both actors point at it.

    // Bob cannot delete Alice's claimed plan.
    let refusal = bob.delete_plan(plan.id).unwrap_err();
    assert!(refusal.to_string().starts_with(INVALID_CLAIM_PREFIX));

    // The owner's own claim dies with the plan; every pointer resets to 0.
    alice.delete_plan(plan.id).unwrap();
    let meta = alice.meta().unwrap();
    assert_eq!(meta.active_plan, 0);
    assert!(meta.active_plans.iter().all(|(_, plan_id)| *plan_id == 0));
    assert!(matches!(alice.plan(plan.id), Err(StoreError::NotFound)));
}

#[test]
fn delete_plan_for_move_deletes_linked_issues_instead_of_detaching() {
    let temp = Temp::new();
    let store = ProjectStore::create_new_with_clock(
        temp.path("move-delete.redb"),
        binding(&temp.path("move-delete.redb"), StoreKind::Project, "move-delete-1"),
        "test",
        clock(),
    )
    .unwrap();
    let plan = store.add_plan("Moving out", 0).unwrap();
    let task = store.add_task(plan.id, "t").unwrap();
    let issue = store.add_issue("follows its task", "", None, task.id).unwrap();
    let unrelated = store.add_issue("stays", "", None, 0).unwrap();

    let summary = store.delete_plan_for_move(plan.id).unwrap();
    assert_eq!(summary.detached_issues, vec![(issue.id, "follows its task".to_owned())]);
    let snapshot = store.snapshot().unwrap();
    assert!(snapshot.issues.iter().all(|i| i.id != issue.id)); // moved, not detached
    assert!(snapshot.issues.iter().any(|i| i.id == unrelated.id)); // untouched
}

#[test]
fn plan_delete_preview_counts_without_mutating() {
    let temp = Temp::new();
    let store = ProjectStore::create_new_with_clock(
        temp.path("preview.redb"),
        binding(&temp.path("preview.redb"), StoreKind::Project, "preview-1"),
        "test",
        clock(),
    )
    .unwrap();
    let plan = store.add_plan("Previewed", 0).unwrap();
    let task = store.add_task(plan.id, "one").unwrap();
    store.add_note(NoteTarget::Task, task.id, "note").unwrap();
    store.add_issue("bug", "", None, task.id).unwrap();

    let summary = store.plan_delete_preview(plan.id).unwrap();
    assert_eq!((summary.tasks, summary.notes), (1, 1));
    assert_eq!(summary.detached_issues.len(), 1);
    assert_eq!(summary.commits_unlinked, 0);
    // Nothing changed.
    assert_eq!(store.snapshot().unwrap().tasks.len(), 1);
    assert!(matches!(
        store.plan_delete_preview(9999),
        Err(StoreError::NotFound)
    ));
}
```

- [ ] Run to see fail: `cargo test -p ptrack-store delete_plan` — expect compile errors: `no method named delete_plan found`.
- [ ] Implement in `crates/ptrack-store/src/project.rs`. Add `BTreeSet` to the existing `use std::collections::BTreeMap;` import (`use std::collections::{BTreeMap, BTreeSet};`). Add the summary type near `MemoryWriteResult`:

```rust
/// What a plan delete destroys (tasks, notes), unlinks (commit records), and
/// detaches (issues). Returned by both the read-only preview and the delete
/// itself so every surface prints the same facts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanDeleteSummary {
    pub plan_id: u64,
    pub title: String,
    pub tasks: usize,
    pub notes: usize,
    pub commits_unlinked: usize,
    /// `(issue id, issue title)` for every issue whose task link is zeroed.
    pub detached_issues: Vec<(u64, String)>,
}
```

Add the pure helper near `cascade`-free helpers at the bottom of the file (alongside `plan_status_can_hold`):

```rust
/// Computes the delete cascade facts from full-collection scans. Pure so the
/// read-only preview and the write-path delete can never disagree.
fn cascade_summary(
    plan: &Plan,
    tasks: &[Task],
    notes: &[Note],
    issues: &[Issue],
    commits: &[Commit],
) -> PlanDeleteSummary {
    let task_ids: BTreeSet<u64> = tasks
        .iter()
        .filter(|task| task.plan_id == plan.id)
        .map(|task| task.id)
        .collect();
    PlanDeleteSummary {
        plan_id: plan.id,
        title: plan.title.clone(),
        tasks: task_ids.len(),
        notes: notes
            .iter()
            .filter(|note| {
                (note.target == NoteTarget::Plan && note.target_id == plan.id)
                    || (note.target == NoteTarget::Task && task_ids.contains(&note.target_id))
            })
            .count(),
        commits_unlinked: commits
            .iter()
            .filter(|commit| commit.plan_id == plan.id || task_ids.contains(&commit.task_id))
            .count(),
        detached_issues: issues
            .iter()
            .filter(|issue| task_ids.contains(&issue.task_id))
            .map(|issue| (issue.id, issue.title.clone()))
            .collect(),
    }
}
```

Add the two methods to `impl ProjectStore` (below `set_plan_milestone`):

```rust
/// Read-only preview of exactly what [`ProjectStore::delete_plan`] would
/// destroy. Ungated: looking is free, only deleting is claim-gated.
pub fn plan_delete_preview(&self, plan_id: u64) -> StoreResult<PlanDeleteSummary> {
    self.read(|transaction| {
        let plan: Plan =
            typed::get(transaction, RecordKey::Id(plan_id))?.ok_or(StoreError::NotFound)?;
        Ok(cascade_summary(
            &plan,
            &typed::scan::<Task>(transaction)?,
            &typed::scan::<Note>(transaction)?,
            &typed::scan::<Issue>(transaction)?,
            &typed::scan::<Commit>(transaction)?,
        ))
    })
}

/// Permanently deletes a plan and cascades in one write transaction: its
/// tasks and their notes are deleted, linked issues survive with their task
/// link zeroed, commit records survive with plan/task references zeroed, and
/// every active-plan pointer (per-actor map and legacy singleton) that named
/// the plan is reset to 0. Claim-gated: deleting a plan claimed by someone
/// else is refused; the deleter's own claim dies with the plan.
pub fn delete_plan(&self, plan_id: u64) -> StoreResult<PlanDeleteSummary> {
    self.delete_plan_inner(plan_id, true)
}

/// The move-phase variant of the delete cascade: identical, except linked
/// issues are deleted rather than detached — the move already duplicated
/// them into the target, and an issue follows its task.
pub fn delete_plan_for_move(&self, plan_id: u64) -> StoreResult<PlanDeleteSummary> {
    self.delete_plan_inner(plan_id, false)
}

fn delete_plan_inner(&self, plan_id: u64, detach_issues: bool) -> StoreResult<PlanDeleteSummary> {
    let now = self.clock.now_local();
    let writer = self.writer_version.clone();
    let actor = self.actor_id().map(str::to_owned);
    self.write(|transaction| {
        let plan = required_write::<Plan>(transaction, RecordKey::Id(plan_id))?;
        require_claim_access(transaction, &plan, actor.as_deref())?;
        let tasks = typed::scan_write::<Task>(transaction)?;
        let notes = typed::scan_write::<Note>(transaction)?;
        let issues = typed::scan_write::<Issue>(transaction)?;
        let commits = typed::scan_write::<Commit>(transaction)?;
        let summary = cascade_summary(&plan, &tasks, &notes, &issues, &commits);
        let task_ids: BTreeSet<u64> = tasks
            .iter()
            .filter(|task| task.plan_id == plan_id)
            .map(|task| task.id)
            .collect();
        for task in &tasks {
            if task.plan_id == plan_id {
                transaction.delete(Collection::Tasks, RecordKey::Id(task.id))?;
            }
        }
        for note in &notes {
            let dead = (note.target == NoteTarget::Plan && note.target_id == plan_id)
                || (note.target == NoteTarget::Task && task_ids.contains(&note.target_id));
            if dead {
                transaction.delete(Collection::Notes, RecordKey::Id(note.id))?;
            }
        }
        for mut issue in issues {
            if task_ids.contains(&issue.task_id) {
                if detach_issues {
                    issue.task_id = 0;
                    issue.updated_at = now;
                    typed::put(transaction, RecordKey::Id(issue.id), &issue)?;
                } else {
                    transaction.delete(Collection::Issues, RecordKey::Id(issue.id))?;
                }
            }
        }
        for mut commit in commits {
            if commit.plan_id == plan_id || task_ids.contains(&commit.task_id) {
                if commit.plan_id == plan_id {
                    commit.plan_id = 0;
                }
                if task_ids.contains(&commit.task_id) {
                    commit.task_id = 0;
                }
                typed::put(transaction, RecordKey::Id(commit.id), &commit)?;
            }
        }
        let mut meta = required_write::<Meta>(transaction, RecordKey::Singleton)?;
        if meta.active_plan == plan_id {
            meta.active_plan = 0;
        }
        for entry in &mut meta.active_plans {
            if entry.1 == plan_id {
                entry.1 = 0;
            }
        }
        stamp_meta(&mut meta, now, writer);
        typed::put(transaction, RecordKey::Singleton, &meta)?;
        transaction.delete(Collection::Plans, RecordKey::Id(plan_id))?;
        Ok(summary)
    })
}
```

- [ ] Export in `crates/ptrack-store/src/lib.rs`: add `PlanDeleteSummary` to the `pub use project::{...}` list (line ~54).
- [ ] Run to pass: `cargo test -p ptrack-store delete_plan plan_delete_preview` then `cargo test -p ptrack-store`.
- [ ] Run all quality gates (see Global Constraints).
- [ ] Commit: `git add -A && git commit -m "feat(store): cascade plan delete with preview summary"`

---

### Task 2: Store subtree export/import engine (same-store copy)

**Files:**
- Modify: `crates/ptrack-store/src/project.rs`
- Modify: `crates/ptrack-store/src/lib.rs` (export)
- Test: `crates/ptrack-store/src/project_test.rs`

**Interfaces:**
- Consumes: same `project.rs` internals as Task 1, plus `transaction.next_id(Collection::...)` and `count_write::<R>(transaction)`.
- Produces:
  - `pub struct PlanSubtree { pub plan: Plan, pub tasks: Vec<Task>, pub notes: Vec<Note>, pub issues: Vec<Issue>, pub commits: Vec<Commit> }` (derives `Clone, Debug`)
  - `pub fn ProjectStore::export_plan_subtree(&self, plan_id: u64) -> StoreResult<PlanSubtree>` (claim-gated; read-only in effect, but runs in the write funnel to reuse the single claim gate and its exact refusal message)
  - `pub fn ProjectStore::import_plan_subtree(&self, subtree: &PlanSubtree, title: Option<String>) -> StoreResult<Plan>` (one write transaction; remints IDs, remaps every reference, arrives unclaimed epoch 0, milestone dropped, holds travel, `title` overrides on arrival)

**Steps:**

- [ ] Write failing tests in `crates/ptrack-store/src/project_test.rs`:

```rust
#[test]
fn export_import_copies_a_plan_subtree_with_reminted_ids_and_no_dangling_refs() {
    let temp = Temp::new();
    let store = ProjectStore::create_new_with_clock(
        temp.path("copy.redb"),
        binding(&temp.path("copy.redb"), StoreKind::Project, "copy-1"),
        "test",
        clock(),
    )
    .unwrap();
    let milestone = store.add_milestone("M1").unwrap();
    let plan = store.add_plan("Original", milestone.id).unwrap();
    store.set_plan_hold(plan.id, Some("waiting".to_owned())).unwrap();
    let task = store.add_task(plan.id, "t1").unwrap();
    store.add_note(NoteTarget::Plan, plan.id, "plan note").unwrap();
    store.add_note(NoteTarget::Task, task.id, "task note").unwrap();
    let issue = store.add_issue("bug", "", None, task.id).unwrap();
    store.add_commit("ccc333", "work", plan.id, task.id).unwrap();

    let subtree = store.export_plan_subtree(plan.id).unwrap();
    assert_eq!(subtree.tasks.len(), 1);
    assert_eq!(subtree.notes.len(), 2);
    assert_eq!(subtree.issues.len(), 1);
    assert_eq!(subtree.commits.len(), 1);

    let copy = store
        .import_plan_subtree(&subtree, Some("Copied".to_owned()))
        .unwrap();
    assert_ne!(copy.id, plan.id);
    assert_eq!(copy.title, "Copied");
    assert_eq!(copy.milestone_id, 0); // milestone link dropped
    assert_eq!(copy.hold_reason.as_deref(), Some("waiting")); // hold travels
    assert_eq!(copy.claim_owner, None); // arrives unclaimed
    assert_eq!(copy.claim_epoch, 0);

    let snapshot = store.snapshot().unwrap();
    let copied_tasks: Vec<_> = snapshot
        .tasks
        .iter()
        .filter(|t| t.plan_id == copy.id)
        .collect();
    assert_eq!(copied_tasks.len(), 1);
    let copied_task = copied_tasks[0];
    assert_ne!(copied_task.id, task.id);
    // Every copied reference points at reminted IDs — zero dangling.
    let copied_issue = snapshot
        .issues
        .iter()
        .find(|i| i.id != issue.id)
        .unwrap();
    assert_eq!(copied_issue.task_id, copied_task.id);
    let copied_commit = snapshot
        .commits
        .iter()
        .find(|c| c.sha == "ccc333" && c.plan_id == copy.id)
        .unwrap();
    assert_eq!(copied_commit.task_id, copied_task.id);
    assert!(
        snapshot
            .notes
            .iter()
            .any(|n| n.target == NoteTarget::Task && n.target_id == copied_task.id)
    );
    assert!(
        snapshot
            .notes
            .iter()
            .any(|n| n.target == NoteTarget::Plan && n.target_id == copy.id)
    );

    // The copy is independent: mutating it leaves the original untouched.
    store.set_task_status(copied_task.id, TaskStatus::Done).unwrap();
    let after = store.snapshot().unwrap();
    assert_eq!(
        after.tasks.iter().find(|t| t.id == task.id).unwrap().status,
        TaskStatus::Todo
    );
    assert_eq!(
        after.plans.iter().find(|p| p.id == plan.id).unwrap().title,
        "Original"
    );
}

#[test]
fn export_plan_subtree_is_claim_gated() {
    let temp = Temp::new();
    let path = temp.path("export-gate.redb");
    let expected = binding(&path, StoreKind::Project, "export-gate-1");
    let alice = ProjectStore::create_new_with_clock(&path, expected.clone(), "test", clock())
        .unwrap()
        .with_actor(Some(ActorIdentity {
            id: "01hzvyekq3s7m8w9x0aaaaaaaa".to_owned(),
            name: "Alice".to_owned(),
        }));
    let bob = ProjectStore::open_existing(&path, &expected, "test")
        .unwrap()
        .with_actor(Some(ActorIdentity {
            id: "01hzvyekq3s7m8w9x0bbbbbbbb".to_owned(),
            name: "Bob".to_owned(),
        }));
    let plan = alice.add_plan("Mine", 0).unwrap();
    alice.use_plan(plan.id, false).unwrap();
    let refusal = bob.export_plan_subtree(plan.id).unwrap_err();
    assert!(refusal.to_string().starts_with(INVALID_CLAIM_PREFIX));
    assert!(alice.export_plan_subtree(plan.id).is_ok());
}
```

- [ ] Run to see fail: `cargo test -p ptrack-store export_plan_subtree export_import` — expect compile errors: `no method named export_plan_subtree found`.
- [ ] Implement in `crates/ptrack-store/src/project.rs` (below `delete_plan`). The subtree type first, near `PlanDeleteSummary`:

```rust
/// One plan with every record that belongs to it, in memory, ready to be
/// inserted into any project store with freshly minted IDs. Not a stored
/// record shape — a carrier between two stores in one process.
#[derive(Clone, Debug)]
pub struct PlanSubtree {
    pub plan: Plan,
    pub tasks: Vec<Task>,
    pub notes: Vec<Note>,
    pub issues: Vec<Issue>,
    pub commits: Vec<Commit>,
}
```

Then the two methods on `impl ProjectStore`:

```rust
/// Collects a plan and everything that travels with it: its tasks, notes on
/// the plan or its tasks, issues linked to its tasks, and commit records
/// referencing the plan or its tasks. Claim-gated like every other content
/// operation on a plan.
pub fn export_plan_subtree(&self, plan_id: u64) -> StoreResult<PlanSubtree> {
    let actor = self.actor_id().map(str::to_owned);
    self.write(|transaction| {
        let plan = required_write::<Plan>(transaction, RecordKey::Id(plan_id))?;
        require_claim_access(transaction, &plan, actor.as_deref())?;
        let tasks: Vec<Task> = typed::scan_write::<Task>(transaction)?
            .into_iter()
            .filter(|task| task.plan_id == plan_id)
            .collect();
        let task_ids: BTreeSet<u64> = tasks.iter().map(|task| task.id).collect();
        let notes = typed::scan_write::<Note>(transaction)?
            .into_iter()
            .filter(|note| {
                (note.target == NoteTarget::Plan && note.target_id == plan_id)
                    || (note.target == NoteTarget::Task && task_ids.contains(&note.target_id))
            })
            .collect();
        let issues = typed::scan_write::<Issue>(transaction)?
            .into_iter()
            .filter(|issue| task_ids.contains(&issue.task_id))
            .collect();
        let commits = typed::scan_write::<Commit>(transaction)?
            .into_iter()
            .filter(|commit| commit.plan_id == plan_id || task_ids.contains(&commit.task_id))
            .collect();
        Ok(PlanSubtree {
            plan,
            tasks,
            notes,
            issues,
            commits,
        })
    })
}

/// Inserts an exported subtree into this store in one write transaction,
/// reminting sequential IDs and remapping every reference. The plan arrives
/// unclaimed (owner `None`, epoch 0), its milestone link is dropped (a
/// milestone is a source-project grouping), hold reasons travel, and `title`
/// replaces the plan title at insert time.
pub fn import_plan_subtree(
    &self,
    subtree: &PlanSubtree,
    title: Option<String>,
) -> StoreResult<Plan> {
    let now = self.clock.now_local();
    let actor = self.actor_id().map(str::to_owned);
    self.write(|transaction| {
        let order = count_write::<Plan>(transaction)?;
        let plan_id = transaction.next_id(Collection::Plans)?;
        let plan = Plan {
            id: plan_id,
            title: title.clone().unwrap_or_else(|| subtree.plan.title.clone()),
            status: subtree.plan.status,
            milestone_id: 0,
            order,
            created_at: subtree.plan.created_at,
            updated_at: now,
            hold_reason: subtree.plan.hold_reason.clone(),
            actor: actor.clone(),
            claim_conflict: false,
            claim_epoch: 0,
            claim_owner: None,
            ulid: None,
        };
        typed::put(transaction, RecordKey::Id(plan_id), &plan)?;
        let mut task_order = count_write::<Task>(transaction)?;
        let mut task_map = BTreeMap::new();
        for task in &subtree.tasks {
            let id = transaction.next_id(Collection::Tasks)?;
            task_map.insert(task.id, id);
            let mut copy = task.clone();
            copy.id = id;
            copy.plan_id = plan_id;
            copy.order = task_order;
            task_order += 1;
            copy.ulid = None;
            typed::put(transaction, RecordKey::Id(id), &copy)?;
        }
        let mapped_task = |source: u64| -> StoreResult<u64> {
            task_map.get(&source).copied().ok_or_else(|| {
                StoreError::InvalidManifest(
                    "imported subtree references a task outside the subtree".to_owned(),
                )
            })
        };
        for note in &subtree.notes {
            let id = transaction.next_id(Collection::Notes)?;
            let mut copy = note.clone();
            copy.id = id;
            copy.target_id = match note.target {
                NoteTarget::Plan => plan_id,
                NoteTarget::Task => mapped_task(note.target_id)?,
                NoteTarget::Project => note.target_id,
            };
            copy.ulid = None;
            typed::put(transaction, RecordKey::Id(id), &copy)?;
        }
        for issue in &subtree.issues {
            let id = transaction.next_id(Collection::Issues)?;
            let mut copy = issue.clone();
            copy.id = id;
            copy.task_id = mapped_task(issue.task_id)?;
            copy.ulid = None;
            typed::put(transaction, RecordKey::Id(id), &copy)?;
        }
        for commit in &subtree.commits {
            let id = transaction.next_id(Collection::Commits)?;
            let mut copy = commit.clone();
            copy.id = id;
            copy.plan_id = if commit.plan_id == subtree.plan.id {
                plan_id
            } else {
                0
            };
            copy.task_id = task_map.get(&commit.task_id).copied().unwrap_or(0);
            copy.ulid = None;
            typed::put(transaction, RecordKey::Id(id), &copy)?;
        }
        Ok(plan)
    })
}
```

Note: child records keep their original `actor` attribution (historical fact); only the arriving plan record is stamped with the operating actor, and the `write()` funnel performs the actor-directory upsert on whichever store the transaction runs against.

- [ ] Export in `crates/ptrack-store/src/lib.rs`: add `PlanSubtree` to the `pub use project::{...}` list.
- [ ] Run to pass: `cargo test -p ptrack-store export` then `cargo test -p ptrack-store`.
- [ ] Run all quality gates.
- [ ] Commit: `git add -A && git commit -m "feat(store): plan subtree export/import engine with ID remapping"`

---

### Task 3: Cross-store copy and crash-window semantics (store-level tests)

**Files:**
- Test: `crates/ptrack-store/src/project_test.rs`
- Modify (only if a test exposes a defect): `crates/ptrack-store/src/project.rs`

**Interfaces:**
- Consumes: `ProjectStore::create_new_with_clock`, `ProjectStore::open_existing`, `export_plan_subtree`, `import_plan_subtree`, `delete_plan` — two independent stores at two paths in one process (legal: each redb file has its own writer; the cutover lease is shared per-host and not involved at this layer).
- Produces: proof of the move contract at the store layer — target-committed-then-source-deleted, and the crash window (target committed + source intact) is a legal, recoverable duplicate state.

**Steps:**

- [ ] Write failing-or-passing tests (they must compile against Task 2's API and pass; if any fails, fix the engine in `project.rs` within this task):

```rust
#[test]
fn cross_store_import_copies_subtree_and_leaves_source_unchanged() {
    let temp = Temp::new();
    let source = ProjectStore::create_new_with_clock(
        temp.path("src.redb"),
        binding(&temp.path("src.redb"), StoreKind::Project, "xsrc-1"),
        "test",
        clock(),
    )
    .unwrap();
    let target = ProjectStore::create_new_with_clock(
        temp.path("dst.redb"),
        binding(&temp.path("dst.redb"), StoreKind::Project, "xdst-1"),
        "test",
        clock(),
    )
    .unwrap();
    // Pre-existing target content so reminted IDs collide if remapping is wrong.
    let existing = target.add_plan("Existing", 0).unwrap();
    target.add_task(existing.id, "existing task").unwrap();

    let plan = source.add_plan("Traveler", 0).unwrap();
    let task = source.add_task(plan.id, "travel task").unwrap();
    source.add_note(NoteTarget::Task, task.id, "note").unwrap();
    source.add_issue("travel bug", "", None, task.id).unwrap();
    source.add_commit("ddd444", "travel commit", plan.id, task.id).unwrap();

    let subtree = source.export_plan_subtree(plan.id).unwrap();
    let arrived = target.import_plan_subtree(&subtree, None).unwrap();
    assert_eq!(arrived.title, "Traveler");
    assert_eq!(arrived.claim_owner, None);
    assert_eq!(arrived.claim_epoch, 0);

    // Target integrity: every imported reference resolves inside the target.
    let snapshot = target.snapshot().unwrap();
    let moved_task = snapshot
        .tasks
        .iter()
        .find(|t| t.plan_id == arrived.id)
        .unwrap();
    assert_eq!(
        snapshot
            .issues
            .iter()
            .filter(|i| i.task_id == moved_task.id)
            .count(),
        1
    );
    assert_eq!(
        snapshot
            .commits
            .iter()
            .filter(|c| c.plan_id == arrived.id && c.task_id == moved_task.id)
            .count(),
        1
    );

    // Source unchanged on copy.
    let source_snapshot = source.snapshot().unwrap();
    assert!(source_snapshot.plans.iter().any(|p| p.id == plan.id));
    assert_eq!(source_snapshot.tasks.len(), 1);
    assert_eq!(source_snapshot.commits.len(), 1);
}

#[test]
fn move_crash_window_leaves_a_visible_duplicate_and_loses_nothing() {
    let temp = Temp::new();
    let source = ProjectStore::create_new_with_clock(
        temp.path("crash-src.redb"),
        binding(&temp.path("crash-src.redb"), StoreKind::Project, "crash-src-1"),
        "test",
        clock(),
    )
    .unwrap();
    let target = ProjectStore::create_new_with_clock(
        temp.path("crash-dst.redb"),
        binding(&temp.path("crash-dst.redb"), StoreKind::Project, "crash-dst-1"),
        "test",
        clock(),
    )
    .unwrap();
    let plan = source.add_plan("Half-moved", 0).unwrap();
    source.add_task(plan.id, "t").unwrap();

    // Phase 1 committed on the target; the process "crashes" before phase 2.
    let subtree = source.export_plan_subtree(plan.id).unwrap();
    let arrived = target.import_plan_subtree(&subtree, None).unwrap();

    // Both sides are fully readable: a duplicate, never a loss.
    assert!(source.snapshot().unwrap().plans.iter().any(|p| p.id == plan.id));
    assert!(target.snapshot().unwrap().plans.iter().any(|p| p.id == arrived.id));

    // Manual cleanup with plan delete on whichever side is unwanted completes the move.
    source.delete_plan(plan.id).unwrap();
    assert!(source.snapshot().unwrap().plans.is_empty());
    assert!(target.snapshot().unwrap().plans.iter().any(|p| p.id == arrived.id));
}
```

- [ ] Run: `cargo test -p ptrack-store cross_store move_crash_window`. Fix `project.rs` only if a test exposes a real defect (rerun the failing test alone first).
- [ ] Run all quality gates.
- [ ] Commit: `git add -A && git commit -m "test(store): cross-store plan copy integrity and move crash-window semantics"`

---

### Task 4: Application service — `plan_lifecycle` port method, target resolution, guards

**Files:**
- Modify: `crates/ptrack-app/src/service.rs`
- Modify: `crates/ptrack-app/src/production.rs` (`RoutedApplication` impl)
- Modify: `crates/ptrack-app/src/lib.rs` (exports)
- Modify: `crates/ptrack-cli/src/dispatch_test.rs` (`FakeApplication` stub — same commit, keeps workspace green)
- Modify: `crates/ptrack-tui/src/runtime_test.rs` (`FakeApplication` stub — same commit)
- Test: `crates/ptrack-app/src/service_test.rs` (guards, single-project paths), `crates/ptrack-app/src/production_test.rs` (cross-project move/copy through a real two-project runtime)

**Interfaces:**
- Consumes: `ProjectStore::{plan_delete_preview, delete_plan, delete_plan_for_move, export_plan_subtree, import_plan_subtree, open_existing, with_actor}`, `GlobalStore::projects`, `crate::identity::load_identity`, `crate::ActiveRuntime::{load, bindings_for_exact_root}` (verified: `ActiveRuntime::load(global_home, writer_version) -> AppResult<Option<Arc<ActiveRuntime>>>` acquires a SHARED cutover lease — stacking on the process's existing shared lease is legal; `bindings_for_exact_root(&Path) -> AppResult<WorkspaceBindings>` errors `AppError::NoProject` for a root absent from the active-generation marker).
- Produces (in `service.rs`, re-exported from `lib.rs`):

```rust
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlanLifecycleRequest {
    DeletePreview { plan_id: u64 },
    Delete { plan_id: u64 },
    Move { plan_id: u64, to: String, rename: Option<String> },
    Copy { plan_id: u64, to: Option<String>, rename: Option<String> },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanTransferSummary {
    pub source_plan_id: u64,
    pub new_plan_id: u64,
    pub title: String,
    pub source_project: String,
    pub target_project: String,
    pub moved: bool,
    pub tasks: usize,
    pub notes: usize,
    pub issues: usize,
    pub commits: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlanLifecycleOutcome {
    Preview(PlanDeleteSummary),
    Deleted(PlanDeleteSummary),
    Transferred(PlanTransferSummary),
}
```

  and the port method `fn plan_lifecycle(&mut self, request: PlanLifecycleRequest) -> AppResult<PlanLifecycleOutcome>;` on `ApplicationPort`, implemented on ALL FIVE impls.

**Steps:**

- [ ] Write failing tests. In `crates/ptrack-app/src/service_test.rs`, using the file's existing `TestDirectory` and `configured(&test, true) -> (LocalApplication, ProjectEndpoint)` helpers (any successful mutation auto-registers the current project in the global registry as `"project"` — its root directory name — via `register_project_best_effort`; add `NoteTarget` to the `ptrack_core` import and `PlanLifecycleOutcome, PlanLifecycleRequest` to the `crate` import):

```rust
#[test]
fn plan_lifecycle_delete_previews_then_deletes_with_summary() {
    let test = TestDirectory::new("lifecycle-delete");
    let (mut application, _endpoint) = configured(&test, true);
    let MutationResult::Plan(plan) = application
        .mutate(Mutation::AddPlan { title: "Doomed".to_owned(), milestone_id: 0 })
        .unwrap()
    else {
        panic!("plan result");
    };
    let MutationResult::Task(task) = application
        .mutate(Mutation::AddTask { plan_id: plan.id, title: "t".to_owned() })
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
    application.mutate(Mutation::SetActivePlan(plan.id)).unwrap();

    let preview = application
        .plan_lifecycle(PlanLifecycleRequest::DeletePreview { plan_id: plan.id })
        .unwrap();
    let PlanLifecycleOutcome::Preview(summary) = preview else {
        panic!("preview outcome");
    };
    assert_eq!(
        (summary.tasks, summary.notes, summary.detached_issues.len()),
        (1, 1, 1)
    );
    assert!(application.snapshot().unwrap().plans.iter().any(|p| p.id == plan.id));

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
        .mutate(Mutation::AddPlan { title: "Stay".to_owned(), milestone_id: 0 })
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
        .mutate(Mutation::AddPlan { title: "Original".to_owned(), milestone_id: 0 })
        .unwrap()
    else {
        panic!("plan result");
    };
    let refusal = application
        .plan_lifecycle(PlanLifecycleRequest::Copy { plan_id: plan.id, to: None, rename: None })
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
        .mutate(Mutation::AddPlan { title: "Lost".to_owned(), milestone_id: 0 })
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
```

In `crates/ptrack-app/src/production_test.rs` (the bootstrap loop is the exact code already at `production_test.rs:701-711`; add `Mutation, MutationResult, PlanLifecycleOutcome, PlanLifecycleRequest` to the file's `crate::` import and `NoteTarget` to its `ptrack_core` import; pass the target as a CANONICAL path string — registry paths are canonical):

```rust
fn bootstrap_two_projects(temp: &Temp) -> (PathBuf, PathBuf, PathBuf) {
    let home = temp.0.join("xfer-home");
    let first = temp.0.join("xfer-first");
    let second = temp.0.join("xfer-second");
    fs::create_dir(&home).unwrap();
    fs::create_dir(&first).unwrap();
    fs::create_dir(&second).unwrap();
    private_directory(&home);
    for root in [&first, &second] {
        let mut application = RoutedApplication::new(home.clone(), first.clone(), "test");
        application
            .initialize(InitRequest {
                root: Some(root.clone()),
                goal: String::new(),
                force: false,
                no_guide: true,
            })
            .unwrap();
    }
    (home, first, second)
}

fn seed_transfer_plan(source: &mut RoutedApplication) -> (u64, u64) {
    let MutationResult::Plan(plan) = source
        .mutate(Mutation::AddPlan { title: "Traveler".to_owned(), milestone_id: 0 })
        .unwrap()
    else {
        panic!("plan result");
    };
    let MutationResult::Task(task) = source
        .mutate(Mutation::AddTask { plan_id: plan.id, title: "t".to_owned() })
        .unwrap()
    else {
        panic!("task result");
    };
    source
        .mutate(Mutation::AddNote {
            target: NoteTarget::Task,
            target_id: task.id,
            body: "n".to_owned(),
        })
        .unwrap();
    source
        .mutate(Mutation::AddIssue {
            title: "bug".to_owned(),
            body: String::new(),
            severity: None,
            task_id: task.id,
        })
        .unwrap();
    source
        .mutate(Mutation::AddCommit {
            sha: "eee555".to_owned(),
            subject: "s".to_owned(),
            plan_id: plan.id,
            task_id: task.id,
        })
        .unwrap();
    (plan.id, task.id)
}

#[test]
fn routed_plan_lifecycle_moves_a_plan_between_two_bootstrapped_projects() {
    let temp = Temp::new();
    let (home, first, second) = bootstrap_two_projects(&temp);
    let mut source = RoutedApplication::new(home.clone(), first, "test");
    let (plan_id, _task_id) = seed_transfer_plan(&mut source);

    let outcome = source
        .plan_lifecycle(PlanLifecycleRequest::Move {
            plan_id,
            to: fs::canonicalize(&second).unwrap().to_string_lossy().into_owned(),
            rename: Some("Landed".to_owned()),
        })
        .unwrap();
    let PlanLifecycleOutcome::Transferred(summary) = outcome else {
        panic!("transfer outcome");
    };
    assert!(summary.moved);
    assert_eq!(summary.title, "Landed");
    assert_ne!(summary.new_plan_id, 0);

    let source_snapshot = source.snapshot().unwrap();
    assert!(source_snapshot.plans.is_empty());
    assert!(source_snapshot.issues.is_empty()); // the issue moved with its task
    assert!(
        source_snapshot
            .commits
            .iter()
            .all(|c| c.plan_id == 0 && c.task_id == 0)
    );

    let mut target = RoutedApplication::new(home, second, "test");
    let target_snapshot = target.snapshot().unwrap();
    let landed = target_snapshot
        .plans
        .iter()
        .find(|p| p.title == "Landed")
        .unwrap();
    assert_eq!(landed.claim_owner, None);
    assert_eq!(landed.claim_epoch, 0);
    let landed_task = target_snapshot
        .tasks
        .iter()
        .find(|t| t.plan_id == landed.id)
        .unwrap();
    assert!(target_snapshot.issues.iter().any(|i| i.task_id == landed_task.id));
    assert!(
        target_snapshot
            .notes
            .iter()
            .any(|n| n.target == NoteTarget::Task && n.target_id == landed_task.id)
    );
    assert!(
        target_snapshot
            .commits
            .iter()
            .any(|c| c.plan_id == landed.id && c.task_id == landed_task.id)
    );
}

#[test]
fn routed_plan_lifecycle_copies_a_plan_and_leaves_the_source_intact() {
    let temp = Temp::new();
    let (home, first, second) = bootstrap_two_projects(&temp);
    let mut source = RoutedApplication::new(home.clone(), first, "test");
    let (plan_id, _task_id) = seed_transfer_plan(&mut source);

    let outcome = source
        .plan_lifecycle(PlanLifecycleRequest::Copy {
            plan_id,
            to: Some(fs::canonicalize(&second).unwrap().to_string_lossy().into_owned()),
            rename: None,
        })
        .unwrap();
    let PlanLifecycleOutcome::Transferred(summary) = outcome else {
        panic!("transfer outcome");
    };
    assert!(!summary.moved);
    assert_eq!(summary.title, "Traveler");

    let source_snapshot = source.snapshot().unwrap();
    assert!(source_snapshot.plans.iter().any(|p| p.id == plan_id));
    assert_eq!(source_snapshot.tasks.len(), 1);
    let mut target = RoutedApplication::new(home, second, "test");
    assert!(
        target
            .snapshot()
            .unwrap()
            .plans
            .iter()
            .any(|p| p.title == "Traveler")
    );
}
```

(If the harness's `Temp` type differs in this file — it uses `Temp` at line 692 — mirror whatever the two-project test at line 690 uses for temp-dir and `private_directory` setup.)

- [ ] Run to see fail: `cargo test -p ptrack-app plan_lifecycle` — expect compile errors: `no method named plan_lifecycle`.
- [ ] Implement in `crates/ptrack-app/src/service.rs`:
  1. Add the three types above (near `Mutation`/`MutationResult`). Import `ptrack_store::{PlanDeleteSummary, ProjectStore}` (ProjectStore already imported).
  2. Add to the `ApplicationPort` trait, after `mutate`: `fn plan_lifecycle(&mut self, request: PlanLifecycleRequest) -> AppResult<PlanLifecycleOutcome>;`
  3. `UnavailableApplication`: `fn plan_lifecycle(&mut self, _request: PlanLifecycleRequest) -> AppResult<PlanLifecycleOutcome> { Err(unavailable()) }`
  4. `LocalApplication` helpers (private, below `register_project_best_effort`):

```rust
/// Finds a registered target project by name or path, exactly as
/// `ptrack projects` prints them. Registry-only: no marker resolution here,
/// so "is this the current project?" can be answered without an active
/// runtime lookup.
fn lookup_registered_project(&self, to: &str) -> AppResult<ProjectRef> {
    let projects = self.with_global(|store| Ok(store.projects()?))?;
    projects
        .into_iter()
        .find(|project| {
            project.name == to || project.path == to || Path::new(&project.path) == Path::new(to)
        })
        .ok_or_else(|| {
            AppError::Message(format!(
                "unknown target project {to:?}; run 'ptrack projects' for registered names and paths"
            ))
        })
}

/// Resolves a registered project to an openable endpoint through the
/// active-generation marker. Only called for a project other than the
/// current one.
fn endpoint_for_registered(&self, project: &ProjectRef) -> AppResult<ProjectEndpoint> {
    let runtime = crate::ActiveRuntime::load(
        &self.bindings.global_home,
        &self.bindings.writer_version,
    )?
    .ok_or_else(|| AppError::Message("active runtime binding is unavailable".to_owned()))?;
    let bindings = runtime
        .bindings_for_exact_root(Path::new(&project.path))
        .map_err(|error| match error {
            AppError::NoProject => AppError::Message(format!(
                "target project {} has no active database binding; run 'ptrack init' inside it once",
                project.path
            )),
            other => other,
        })?;
    bindings.project.ok_or(AppError::NoProject)
}

fn transfer_plan(
    &self,
    plan_id: u64,
    to: Option<&str>,
    rename: Option<String>,
    is_move: bool,
) -> AppResult<PlanLifecycleOutcome> {
    let source = self.project()?.clone();
    let target_ref = to.map(|to| self.lookup_registered_project(to)).transpose()?;
    let same_project = target_ref
        .as_ref()
        .is_none_or(|project| Path::new(&project.path) == source.root.as_path());
    if is_move && same_project {
        return Err(AppError::Message(
            "target project is the current project; rename it in place with 'ptrack plan rename'"
                .to_owned(),
        ));
    }
    if !is_move && same_project && rename.is_none() {
        return Err(AppError::Message(
            "copying into the same project requires --as <new title>".to_owned(),
        ));
    }
    let target = if same_project {
        None
    } else {
        Some(self.endpoint_for_registered(
            target_ref.as_ref().expect("cross-project transfer has a registry entry"),
        )?)
    };
    let actor = self.with_global(crate::identity::load_identity)?;
    let writer_version = self.bindings.writer_version.clone();
    self.with_project(|store| {
        let subtree = store.export_plan_subtree(plan_id)?;
        let (tasks, notes, issues, commits) = (
            subtree.tasks.len(),
            subtree.notes.len(),
            subtree.issues.len(),
            subtree.commits.len(),
        );
        let (new_plan, target_label) = if same_project {
            (
                store.import_plan_subtree(&subtree, rename.clone())?,
                project_label(&source.root),
            )
        } else {
            let endpoint = target.as_ref().expect("cross-project transfer has a target");
            let target_store =
                ProjectStore::open_existing(&endpoint.database, &endpoint.binding, &writer_version)
                    .map_err(|error| target_open_error(&endpoint.root, &error))?
                    .with_actor(actor.clone());
            let plan = target_store.import_plan_subtree(&subtree, rename.clone())?;
            drop(target_store);
            (plan, project_label(&endpoint.root))
        };
        if is_move {
            // Only after the target transaction has committed. Issues that
            // traveled are deleted here, not detached — they follow their task.
            store.delete_plan_for_move(plan_id)?;
        }
        Ok(PlanLifecycleOutcome::Transferred(PlanTransferSummary {
            source_plan_id: plan_id,
            new_plan_id: new_plan.id,
            title: new_plan.title,
            source_project: project_label(&source.root),
            target_project: target_label,
            moved: is_move,
            tasks,
            notes,
            issues,
            commits,
        }))
    })
}
```

     with two free functions at the bottom of `service.rs`:

```rust
/// A registered project's short display label: its directory name, falling
/// back to the whole path when it has none.
fn project_label(root: &Path) -> String {
    root.file_name()
        .and_then(|name| name.to_str())
        .map_or_else(|| root.display().to_string(), str::to_owned)
}

/// Fail-closed target-open refusal: the store's own manifest/schema message,
/// plus the upgrade hint the spec requires when the target was written by a
/// newer build.
fn target_open_error(root: &Path, error: &ptrack_store::StoreError) -> AppError {
    let hint = if matches!(
        error,
        ptrack_store::StoreError::UnsupportedSchemaVersion { .. }
            | ptrack_store::StoreError::InvalidManifest(_)
    ) {
        "; upgrade ptrack for that project and try again"
    } else {
        ""
    };
    AppError::Message(format!(
        "cannot open target project {}: {error}{hint}",
        root.display()
    ))
}
```

  5. `LocalApplication::plan_lifecycle`:

```rust
fn plan_lifecycle(&mut self, request: PlanLifecycleRequest) -> AppResult<PlanLifecycleOutcome> {
    match request {
        PlanLifecycleRequest::DeletePreview { plan_id } => self.with_project(|store| {
            Ok(PlanLifecycleOutcome::Preview(store.plan_delete_preview(plan_id)?))
        }),
        PlanLifecycleRequest::Delete { plan_id } => self.with_project(|store| {
            Ok(PlanLifecycleOutcome::Deleted(store.delete_plan(plan_id)?))
        }),
        PlanLifecycleRequest::Move { plan_id, to, rename } => {
            self.transfer_plan(plan_id, Some(&to), rename, true)
        }
        PlanLifecycleRequest::Copy { plan_id, to, rename } => {
            self.transfer_plan(plan_id, to.as_deref(), rename, false)
        }
    }
}
```

  6. `RoutedApplication` in `production.rs`: `fn plan_lifecycle(&mut self, request: PlanLifecycleRequest) -> AppResult<PlanLifecycleOutcome> { self.local()?.plan_lifecycle(request) }` (add `PlanLifecycleOutcome, PlanLifecycleRequest` to its `crate::` imports).
  7. `crates/ptrack-app/src/lib.rs`: add `PlanLifecycleOutcome, PlanLifecycleRequest, PlanTransferSummary` to the `pub use service::{...}` list and add `pub use ptrack_store::{PlanDeleteSummary, PlanSubtree};` next to the existing `pub use ptrack_store::ActorIdentity;`.
  8. `crates/ptrack-cli/src/dispatch_test.rs` `FakeApplication`: add two fields — `lifecycle_requests: Vec<PlanLifecycleRequest>` and `lifecycle_results: Vec<AppResult<PlanLifecycleOutcome>>` — in the struct's existing `Default`-constructed field style, and implement the port method to record the request and pop a queued result:

```rust
fn plan_lifecycle(&mut self, request: PlanLifecycleRequest) -> AppResult<PlanLifecycleOutcome> {
    self.lifecycle_requests.push(request);
    self.lifecycle_results
        .pop()
        .unwrap_or_else(|| Err(AppError::NotImplemented("test plan lifecycle")))
}
```

  9. `crates/ptrack-tui/src/runtime_test.rs` `FakeApplication`: `fn plan_lifecycle(&mut self, _request: PlanLifecycleRequest) -> AppResult<PlanLifecycleOutcome> { Err(AppError::NotImplemented("test plan lifecycle")) }` (import the two types).
- [ ] Run to pass: `cargo test -p ptrack-app plan_lifecycle` and `cargo test -p ptrack-app routed_plan_lifecycle`, then `cargo test --workspace --all-targets --no-fail-fast`.
- [ ] Run all quality gates.
- [ ] Commit: `git add -A && git commit -m "feat(app): plan_lifecycle port method with cross-project target resolution"`

---

### Task 5: CLI — `plan delete|move|copy` across all five registration lists

**Files:**
- Modify: `crates/ptrack-cli/src/tree.rs`
- Modify: `crates/ptrack-cli/src/command.rs`
- Modify: `crates/ptrack-cli/src/parse.rs`
- Modify: `crates/ptrack-cli/src/help.rs`
- Modify: `crates/ptrack-cli/src/dispatch.rs`
- Test: `crates/ptrack-cli/src/dispatch_test.rs`, `crates/ptrack-cli/src/parse_test.rs`

**Interfaces:**
- Consumes: `ApplicationPort::plan_lifecycle`, `PlanLifecycleRequest`, `PlanLifecycleOutcome`, `PlanDeleteSummary`, `PlanTransferSummary` (all exported by ptrack-app in Task 4); existing `claim_error` prefix-stripping helper in `dispatch.rs`.
- Produces: `ptrack plan delete <id> [--force]`, `ptrack plan move <id> --to <project> [--as <title>]`, `ptrack plan copy <id> [--to <project>] [--as <title>]`.

**Steps:**

- [ ] Write failing tests. In `crates/ptrack-cli/src/dispatch_test.rs` (use the file's existing `run(["ptrack", ...].map(str::to_owned), &mut application, io)` pattern and the `FakeApplication` extended in Task 4):

```rust
#[test]
fn plan_delete_without_force_prints_preview_and_refuses() {
    let mut application = FakeApplication::default();
    application.lifecycle_results.push(Ok(PlanLifecycleOutcome::Preview(PlanDeleteSummary {
        plan_id: 3,
        title: "Doomed".to_owned(),
        tasks: 2,
        notes: 1,
        commits_unlinked: 4,
        detached_issues: vec![(7, "crash on save".to_owned())],
    })));
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let result = run(
        ["ptrack", "plan", "delete", "3"].map(str::to_owned),
        &mut application,
        test_io(&mut stdout, &mut stderr),
    );
    assert_eq!(
        application.lifecycle_requests,
        vec![PlanLifecycleRequest::DeletePreview { plan_id: 3 }]
    );
    let text = String::from_utf8(stdout).unwrap();
    assert!(text.contains("plan #3 \"Doomed\": 2 task(s), 1 note(s), 1 issue link(s), 4 commit record(s)"));
    assert!(text.contains("would detach issue #7 \"crash on save\""));
    assert!(result.unwrap_err().to_string().contains("--force"));
}

#[test]
fn plan_delete_with_force_deletes_and_prints_the_same_summary() {
    let mut application = FakeApplication::default();
    application.lifecycle_results.push(Ok(PlanLifecycleOutcome::Deleted(PlanDeleteSummary {
        plan_id: 3,
        title: "Doomed".to_owned(),
        tasks: 2,
        notes: 1,
        commits_unlinked: 0,
        detached_issues: vec![(7, "crash on save".to_owned())],
    })));
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    run(
        ["ptrack", "plan", "delete", "3", "--force"].map(str::to_owned),
        &mut application,
        test_io(&mut stdout, &mut stderr),
    )
    .unwrap();
    assert_eq!(
        application.lifecycle_requests,
        vec![PlanLifecycleRequest::Delete { plan_id: 3 }]
    );
    let text = String::from_utf8(stdout).unwrap();
    assert!(text.contains("detached issue #7 \"crash on save\""));
    assert!(text.contains("plan #3 deleted"));
}

#[test]
fn plan_move_requires_to_and_prints_both_projects_and_the_new_id() {
    let mut application = FakeApplication::default();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let missing = run(
        ["ptrack", "plan", "move", "3"].map(str::to_owned),
        &mut application,
        test_io(&mut stdout, &mut stderr),
    );
    assert!(missing.unwrap_err().to_string().contains("--to"));
    assert!(application.lifecycle_requests.is_empty());

    application.lifecycle_results.push(Ok(PlanLifecycleOutcome::Transferred(PlanTransferSummary {
        source_plan_id: 3,
        new_plan_id: 9,
        title: "Landed".to_owned(),
        source_project: "alpha".to_owned(),
        target_project: "beta".to_owned(),
        moved: true,
        tasks: 2,
        notes: 1,
        issues: 1,
        commits: 4,
    })));
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    run(
        ["ptrack", "plan", "move", "3", "--to", "beta", "--as", "Landed"].map(str::to_owned),
        &mut application,
        test_io(&mut stdout, &mut stderr),
    )
    .unwrap();
    assert_eq!(
        application.lifecycle_requests,
        vec![PlanLifecycleRequest::Move {
            plan_id: 3,
            to: "beta".to_owned(),
            rename: Some("Landed".to_owned()),
        }]
    );
    let text = String::from_utf8(stdout).unwrap();
    assert!(text.contains(
        "moved plan #3 from alpha to beta: now plan #9 \"Landed\" (2 tasks, 1 notes, 1 issues, 4 commits)"
    ));
}

#[test]
fn plan_copy_passes_optional_target_and_rename_through() {
    let mut application = FakeApplication::default();
    application.lifecycle_results.push(Ok(PlanLifecycleOutcome::Transferred(PlanTransferSummary {
        source_plan_id: 3,
        new_plan_id: 12,
        title: "Second".to_owned(),
        source_project: "alpha".to_owned(),
        target_project: "alpha".to_owned(),
        moved: false,
        tasks: 0,
        notes: 0,
        issues: 0,
        commits: 0,
    })));
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    run(
        ["ptrack", "plan", "copy", "3", "--as", "Second"].map(str::to_owned),
        &mut application,
        test_io(&mut stdout, &mut stderr),
    )
    .unwrap();
    assert_eq!(
        application.lifecycle_requests,
        vec![PlanLifecycleRequest::Copy {
            plan_id: 3,
            to: None,
            rename: Some("Second".to_owned()),
        }]
    );
    let text = String::from_utf8(stdout).unwrap();
    assert!(text.contains("copied plan #3 to alpha: new plan #12 \"Second\""));
}
```

(If `dispatch_test.rs` has no `test_io` helper, use the file's existing way of constructing `Io` for `run` — copy the exact construction from `capability_mcp_uses_stdin_as_sole_protocol_input_and_emits_no_cli_text`.)

In `crates/ptrack-cli/src/parse_test.rs`, add registration-level tests in the file's preflight-assert style:

```rust
#[test]
fn plan_lifecycle_leaves_validate_flags_and_arg_counts() {
    let error = preflight(vec!["ptrack".into(), "plan".into(), "delete".into()])
        .expect_err("missing id");
    assert_eq!(error.to_string(), "accepts 1 arg(s), received 0");
    let result = preflight(vec![
        "ptrack".into(),
        "plan".into(),
        "delete".into(),
        "3".into(),
        "--force".into(),
    ])
    .expect("delete parses");
    assert!(matches!(result, Preflight::Run { path, .. } if path == ["plan", "delete"]));
    let result = preflight(vec![
        "ptrack".into(),
        "plan".into(),
        "move".into(),
        "3".into(),
        "--to".into(),
        "beta".into(),
    ])
    .expect("move parses");
    assert!(matches!(result, Preflight::Run { path, .. } if path == ["plan", "move"]));
    let error = preflight(vec![
        "ptrack".into(),
        "plan".into(),
        "move".into(),
        "3".into(),
        "--bogus".into(),
        "x".into(),
    ])
    .expect_err("unknown flag");
    assert_eq!(error.to_string(), "unknown flag: --bogus");
    let result = preflight(vec![
        "ptrack".into(),
        "plan".into(),
        "copy".into(),
        "3".into(),
        "--to".into(),
        "beta".into(),
        "--as".into(),
        "New".into(),
    ])
    .expect("copy parses");
    assert!(matches!(result, Preflight::Run { path, .. } if path == ["plan", "copy"]));
}
```

- [ ] Run to see fail: `cargo test -p ptrack-cli plan_delete plan_move plan_copy plan_lifecycle_leaves` — expect preflight errors (`unknown command "delete" for "ptrack plan"` / help fallthrough) and compile errors for the new imports.
- [ ] Register in the five lists:
  1. `tree.rs` plan group: children list becomes `&["add", "list", "show", "done", "use", "release", "rename", "hold", "resume", "delete", "move", "copy"]` and add
     `.mut_subcommand("delete", |c| c.arg(positional("id", 1)).arg(flag("force")))`
     `.mut_subcommand("move", |c| c.arg(positional("id", 1)).args([option("to"), option("as")]))`
     `.mut_subcommand("copy", |c| c.arg(positional("id", 1)).args([option("to"), option("as")]))`.
  2. `command.rs` `LEAVES`: `leaf(&["plan", "delete"], ArgCount::Exact(1))`, `leaf(&["plan", "move"], ArgCount::Exact(1))`, `leaf(&["plan", "copy"], ArgCount::Exact(1))` next to the other plan leaves.
  3. `parse.rs` `GROUPS` plan entry: append `"delete", "move", "copy"` to its children slice.
  4. `parse.rs` `flag_names`: add arms `["plan", "delete"] => &[("force", false)],` and `["plan", "move" | "copy"] => &[("to", true), ("as", true)],`.
  5. `help.rs`: `PLAN_CHILDREN` gains (alphabetical) `child("copy", "Copy a plan subtree into another project or duplicate it here")`, `child("delete", "Permanently delete a plan and its tasks and notes")`, `child("move", "Move a plan subtree to another registered project")`; `plan_leaf` gains arms:

```rust
"delete" => leaf_spec(
    "plan delete <id>",
    "Permanently delete a plan, its tasks, and their notes (issues detach, commits keep an unlinked audit record)",
    &[
        flag("    --force", "actually delete; without it the command only prints what would be destroyed"),
        HELP_FLAG,
    ],
),
"move" => leaf_spec(
    "plan move <id> --to <project>",
    "Move a plan subtree to another registered project (arrives unclaimed; --as renames on arrival)",
    &[
        flag("    --as string", "rename the plan on arrival"),
        HELP_FLAG,
        flag("    --to string", "target project name or path as shown by 'ptrack projects' (required)"),
    ],
),
"copy" => leaf_spec(
    "plan copy <id>",
    "Copy a plan subtree into another project, or duplicate it here (--as required without --to)",
    &[
        flag("    --as string", "title for the copy (required when copying within this project)"),
        HELP_FLAG,
        flag("    --to string", "target project name or path (default: this project)"),
    ],
),
```

     and `group_children("plan")` becomes `Some(&["add", "copy", "delete", "done", "hold", "list", "move", "release", "rename", "resume", "show", "use"])`.
- [ ] Dispatch in `dispatch.rs` — extend the `plan(...)` match (import `PlanLifecycleOutcome, PlanLifecycleRequest` from `ptrack_app`):

```rust
"delete" => {
    let id = parse_u64(first(matches, "id")?)?;
    let force = matches.get_flag("force");
    let request = if force {
        PlanLifecycleRequest::Delete { plan_id: id }
    } else {
        PlanLifecycleRequest::DeletePreview { plan_id: id }
    };
    let outcome = application.plan_lifecycle(request).map_err(claim_error)?;
    let (summary, deleted) = match outcome {
        PlanLifecycleOutcome::Preview(summary) => (summary, false),
        PlanLifecycleOutcome::Deleted(summary) => (summary, true),
        PlanLifecycleOutcome::Transferred(_) => return Err(internal_result()),
    };
    output::line(
        io.stdout,
        format_args!(
            "plan #{} \"{}\": {} task(s), {} note(s), {} issue link(s), {} commit record(s)",
            summary.plan_id,
            summary.title,
            summary.tasks,
            summary.notes,
            summary.detached_issues.len(),
            summary.commits_unlinked
        ),
    )?;
    for (issue_id, title) in &summary.detached_issues {
        let verb = if deleted { "detached" } else { "would detach" };
        output::line(io.stdout, format_args!("{verb} issue #{issue_id} \"{title}\""))?;
    }
    if deleted {
        output::line(io.stdout, format_args!("plan #{} deleted", summary.plan_id))?;
    } else {
        return Err(CliError::message(format!(
            "refusing to delete plan #{} without --force",
            summary.plan_id
        )));
    }
}
"move" => {
    let id = parse_u64(first(matches, "id")?)?;
    let Some(to) = option(matches, "to").cloned() else {
        return Err(CliError::message(
            "pass the target project with --to <project>",
        ));
    };
    let rename = option(matches, "as").cloned();
    let outcome = application
        .plan_lifecycle(PlanLifecycleRequest::Move { plan_id: id, to, rename })
        .map_err(claim_error)?;
    let PlanLifecycleOutcome::Transferred(summary) = outcome else {
        return Err(internal_result());
    };
    output::line(
        io.stdout,
        format_args!(
            "moved plan #{} from {} to {}: now plan #{} \"{}\" ({} tasks, {} notes, {} issues, {} commits)",
            summary.source_plan_id,
            summary.source_project,
            summary.target_project,
            summary.new_plan_id,
            summary.title,
            summary.tasks,
            summary.notes,
            summary.issues,
            summary.commits
        ),
    )?;
}
"copy" => {
    let id = parse_u64(first(matches, "id")?)?;
    let to = option(matches, "to").cloned();
    let rename = option(matches, "as").cloned();
    let outcome = application
        .plan_lifecycle(PlanLifecycleRequest::Copy { plan_id: id, to, rename })
        .map_err(claim_error)?;
    let PlanLifecycleOutcome::Transferred(summary) = outcome else {
        return Err(internal_result());
    };
    output::line(
        io.stdout,
        format_args!(
            "copied plan #{} to {}: new plan #{} \"{}\"",
            summary.source_plan_id,
            summary.target_project,
            summary.new_plan_id,
            summary.title
        ),
    )?;
}
```

- [ ] Run to pass: `cargo test -p ptrack-cli`.
- [ ] Run all quality gates (help output snapshot tests, if any, will guide exact spacing — adjust the help specs to match the file's alignment conventions, not the other way around).
- [ ] Commit: `git add -A && git commit -m "feat(cli): plan delete/move/copy with --force, --to, and --as"`

---

### Task 6: Desktop runtime — five new bridge commands

**Files:**
- Modify: `crates/ptrack-app/src/desktop_runtime.rs`
- Test: `crates/ptrack-app/src/desktop_runtime_test.rs`

**Interfaces:**
- Consumes: `ApplicationPort::{mutate, plan_lifecycle, projects}` through `lock(&self.application)`; existing helpers `require_argument_count`, `u64_arg`, `string_arg`, `bool_arg`, `trimmed_nonempty`, `require_generation`, `value`, `json!`.
- Produces: allowlist entries `CopyPlanV1`, `DeletePlanV1`, `ListProjectsV1`, `MovePlanV1`, `RenamePlanV1` (COMMANDS grows 88 → 93, kept sorted); five `BoundDesktopWorkspace::invoke` arms. The GUI's delete preview IS `DeletePlanV1` with `force=false` (mirrors the CLI's force-less ceremony; keeps the allowlist at exactly +5 as the spec counts it). Every claim/hold/guard refusal surfaces as the `AppError` message string verbatim — no prefix stripping in the desktop layer.

**Steps:**

- [ ] Write failing tests in `crates/ptrack-app/src/desktop_runtime_test.rs` (use the existing `bound_workspace(&directory)` harness, generation 7):

```rust
#[test]
fn desktop_plan_lifecycle_commands_rename_preview_delete_and_copy_within() {
    let directory = TestDirectory::new("plan-lifecycle-commands");
    let workspace = bound_workspace(&directory); // seeded: plan "Desktop" + task + note
    let plan_id = 1_u64;

    // Rename.
    workspace
        .invoke("RenamePlanV1", &[json!(7), json!(plan_id), json!("Renamed")])
        .unwrap();
    // Preview (force=false): counts, nothing deleted.
    let preview = workspace
        .invoke("DeletePlanV1", &[json!(7), json!(plan_id), json!(false)])
        .unwrap();
    assert_eq!(preview["preview"], json!(true));
    assert_eq!(preview["summary"]["title"], json!("Renamed"));
    assert_eq!(preview["summary"]["tasks"], json!(1));
    assert_eq!(preview["summary"]["notes"], json!(1));

    // Copy within the project requires a new title; empty target + empty title fails.
    let refusal = workspace
        .invoke("CopyPlanV1", &[json!(7), json!(plan_id), json!(""), json!("")])
        .unwrap_err();
    assert!(refusal.to_string().contains("--as"));
    let copied = workspace
        .invoke("CopyPlanV1", &[json!(7), json!(plan_id), json!(""), json!("Second")])
        .unwrap();
    assert_eq!(copied["summary"]["title"], json!("Second"));
    assert_eq!(copied["summary"]["moved"], json!(false));

    // Delete (force=true) removes it.
    let deleted = workspace
        .invoke("DeletePlanV1", &[json!(7), json!(plan_id), json!(true)])
        .unwrap();
    assert_eq!(deleted["preview"], json!(false));
    let missing = workspace
        .invoke("DeletePlanV1", &[json!(7), json!(plan_id), json!(false)])
        .unwrap_err();
    assert!(missing.to_string().contains("not found"));

    // Move to an unregistered project surfaces the guard message verbatim.
    let unknown = workspace
        .invoke("MovePlanV1", &[json!(7), json!(2), json!("/no/such/project"), json!("")])
        .unwrap_err();
    assert!(unknown.to_string().contains("ptrack projects"));

    // ListProjectsV1 answers with the registry (possibly empty in this harness).
    let projects = workspace.invoke("ListProjectsV1", &[json!(7)]).unwrap();
    assert!(projects["projects"].is_array());
}
```

  Also update the freeze-fixture test `desktop_command_allowlist_is_exact_sorted_unique_and_byte_bounded`: insert `"CopyPlanV1"`, `"DeletePlanV1"`, `"ListProjectsV1"`, `"MovePlanV1"`, `"RenamePlanV1"` at their sorted positions and fix its `// Full 87-command freeze fixture` comment to `// Full 93-command freeze fixture`.
- [ ] Run to see fail: `cargo test -p ptrack-app desktop_plan_lifecycle desktop_command_allowlist` — expect the allowlist assertion mismatch and `unavailable` errors for the new methods.
- [ ] Implement in `desktop_runtime.rs`:
  1. `COMMANDS`: change to `[&str; 93]`, insert the five names sorted (`CopyPlanV1` after `CloseTerminalV2`; `DeletePlanV1` after `CreateTerminalV2`; `ListProjectsV1` after `LaunchLinkedAgentV2`; `MovePlanV1` after `MoveTaskV3`; `RenamePlanV1` after `RemoveCapabilityV2`). Update the doc comment to `/// Exact current 93-method desktop bridge command allowlist.`
  2. Add `PlanLifecycleOutcome, PlanLifecycleRequest` to the `crate::{...}` import.
  3. Add helpers near the other free functions at the bottom:

```rust
/// Treats an empty or blank bridge string argument as "not provided".
fn optional_string(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

fn delete_summary_json(summary: &ptrack_store::PlanDeleteSummary) -> Value {
    json!({
        "planId": summary.plan_id,
        "title": summary.title,
        "tasks": summary.tasks,
        "notes": summary.notes,
        "commits": summary.commits_unlinked,
        "detachedIssues": summary
            .detached_issues
            .iter()
            .map(|(id, title)| json!({ "id": id, "title": title }))
            .collect::<Vec<_>>(),
    })
}

fn transfer_summary_json(summary: &crate::PlanTransferSummary) -> Value {
    json!({
        "sourcePlanId": summary.source_plan_id,
        "newPlanId": summary.new_plan_id,
        "title": summary.title,
        "sourceProject": summary.source_project,
        "targetProject": summary.target_project,
        "moved": summary.moved,
        "tasks": summary.tasks,
        "notes": summary.notes,
        "issues": summary.issues,
        "commits": summary.commits,
    })
}
```

  4. Add five arms to `BoundDesktopWorkspace::invoke` (next to `"RenameTask" | "RenameTaskV2"`):

```rust
"RenamePlanV1" => {
    require_argument_count(method, arguments, 3)?;
    let generation = u64_arg(arguments, 0)?;
    self.require_generation(generation)?;
    let title = trimmed_nonempty(string_arg(arguments, 2)?, "plan title cannot be empty")?;
    lock(&self.application).mutate(Mutation::SetPlanTitle {
        id: u64_arg(arguments, 1)?,
        title,
    })?;
    Ok(json!({ "generation": self.generation }))
}
"DeletePlanV1" => {
    require_argument_count(method, arguments, 3)?;
    let generation = u64_arg(arguments, 0)?;
    self.require_generation(generation)?;
    let plan_id = u64_arg(arguments, 1)?;
    let request = if bool_arg(arguments, 2)? {
        PlanLifecycleRequest::Delete { plan_id }
    } else {
        PlanLifecycleRequest::DeletePreview { plan_id }
    };
    let outcome = lock(&self.application).plan_lifecycle(request)?;
    let (summary, preview) = match outcome {
        PlanLifecycleOutcome::Preview(summary) => (summary, true),
        PlanLifecycleOutcome::Deleted(summary) => (summary, false),
        PlanLifecycleOutcome::Transferred(_) => return Err(unavailable("plan delete result")),
    };
    Ok(json!({
        "generation": self.generation,
        "preview": preview,
        "summary": delete_summary_json(&summary),
    }))
}
"MovePlanV1" => {
    require_argument_count(method, arguments, 4)?;
    let generation = u64_arg(arguments, 0)?;
    self.require_generation(generation)?;
    let outcome = lock(&self.application).plan_lifecycle(PlanLifecycleRequest::Move {
        plan_id: u64_arg(arguments, 1)?,
        to: string_arg(arguments, 2)?.to_owned(),
        rename: optional_string(string_arg(arguments, 3)?),
    })?;
    let PlanLifecycleOutcome::Transferred(summary) = outcome else {
        return Err(unavailable("plan move result"));
    };
    Ok(json!({ "generation": self.generation, "summary": transfer_summary_json(&summary) }))
}
"CopyPlanV1" => {
    require_argument_count(method, arguments, 4)?;
    let generation = u64_arg(arguments, 0)?;
    self.require_generation(generation)?;
    let outcome = lock(&self.application).plan_lifecycle(PlanLifecycleRequest::Copy {
        plan_id: u64_arg(arguments, 1)?,
        to: optional_string(string_arg(arguments, 2)?),
        rename: optional_string(string_arg(arguments, 3)?),
    })?;
    let PlanLifecycleOutcome::Transferred(summary) = outcome else {
        return Err(unavailable("plan copy result"));
    };
    Ok(json!({ "generation": self.generation, "summary": transfer_summary_json(&summary) }))
}
"ListProjectsV1" => {
    require_argument_count(method, arguments, 1)?;
    let generation = u64_arg(arguments, 0)?;
    self.require_generation(generation)?;
    let projects = lock(&self.application).projects()?;
    let current = self.endpoint.root.to_string_lossy().into_owned();
    Ok(json!({
        "generation": self.generation,
        "projects": projects
            .iter()
            .map(|project| json!({
                "name": project.name,
                "path": project.path,
                "current": project.path == current,
            }))
            .collect::<Vec<_>>(),
    }))
}
```

- [ ] Run to pass: `cargo test -p ptrack-app desktop_`.
- [ ] Run all quality gates.
- [ ] Commit: `git add -A && git commit -m "feat(desktop): RenamePlanV1/DeletePlanV1/MovePlanV1/CopyPlanV1/ListProjectsV1 bridge commands"`

---

### Task 7: Frontend — plan context menu, inline rename, delete/move/copy dialogs

**Files:**
- Create: `frontend/src/workspace/plan-lifecycle.ts`
- Create: `frontend/src/workspace/plan-lifecycle.test.ts`
- Modify: `frontend/src/tauri-bridge.js` (COMMANDS list)
- Modify: `frontend/src/tauri-bridge.test.js` (exact COMMANDS fixture)
- Modify: `frontend/index.html` (plan dialog markup)
- Modify: `frontend/src/app.js` (wiring)
- Modify: `frontend/src/style.css` (menu + dialog styles)

**Interfaces:**
- Consumes: `api()` bridge methods `RenamePlanV1(generation, planId, title)`, `DeletePlanV1(generation, planId, force)`, `MovePlanV1(generation, planId, targetPath, newTitle)`, `CopyPlanV1(generation, planId, targetPath, newTitle)`, `ListProjectsV1(generation)`; `loadSnapshot(planId, ...)` for refresh; the sidebar plan renderer (the block with `sidebar-plan-hold`, app.js ~line 1500) and the board header element.
- Produces: pure-logic module + tests, and imperative wiring. Errors returned by the bridge (claim refusals, guard messages, schema-gate messages) are rendered verbatim in the dialog's error region.

**Steps:**

- [ ] Add the five command names to `frontend/src/tauri-bridge.js` `COMMANDS` (sorted: `"CopyPlanV1"` after `"CloseTerminalV2"`, `"DeletePlanV1"` after `"CreateTerminalV2"`, `"ListProjectsV1"` after `"LaunchLinkedAgentV2"`, `"MovePlanV1"` after `"MoveTaskV3"`, `"RenamePlanV1"` after `"RemoveCapabilityV2"`) and mirror the same five entries in the exact-array fixture in `frontend/src/tauri-bridge.test.js`. Run to see the fixture test fail first: `npm --prefix frontend test -- tauri-bridge`.
- [ ] Write the pure module `frontend/src/workspace/plan-lifecycle.ts`:

```ts
export type PlanLifecycleAction = "rename" | "delete" | "move" | "copy";

export interface PlanMenuItem {
  action: PlanLifecycleAction;
  label: string;
  destructive: boolean;
}

/** The plan context menu, identical for the sidebar and the board header. */
export function planMenuItems(): PlanMenuItem[] {
  return [
    { action: "rename", label: "Rename", destructive: false },
    { action: "move", label: "Move to project…", destructive: false },
    { action: "copy", label: "Copy…", destructive: false },
    { action: "delete", label: "Delete…", destructive: true },
  ];
}

export interface DeletePreviewSummary {
  planId: number;
  title: string;
  tasks: number;
  notes: number;
  commits: number;
  detachedIssues: { id: number; title: string }[];
}

/** Human sentence for the delete confirmation body, from the preview call. */
export function deleteConfirmationText(summary: DeletePreviewSummary): string {
  const parts = [
    `${summary.tasks} task${summary.tasks === 1 ? "" : "s"}`,
    `${summary.notes} note${summary.notes === 1 ? "" : "s"}`,
  ];
  const issues = summary.detachedIssues.length;
  let text = `Deleting “${summary.title}” permanently removes ${parts.join(" and ")}.`;
  if (issues > 0) {
    text += ` ${issues} linked issue${issues === 1 ? "" : "s"} will be detached and kept.`;
  }
  if (summary.commits > 0) {
    text += ` ${summary.commits} commit record${summary.commits === 1 ? "" : "s"} stay as audit history with their links cleared.`;
  }
  return text;
}

export interface ProjectChoice {
  name: string;
  path: string;
  current: boolean;
}

export interface TransferDialogState {
  mode: "move" | "copy";
  projects: ProjectChoice[];
  targetPath: string; // "" until the user picks
  title: string; // optional new title field
}

/**
 * OK stays disabled until the dialog state is submittable:
 * - move: a non-current target must be chosen;
 * - copy: any target works, but landing in the current project (explicitly or
 *   by leaving the picker empty) requires a new title.
 */
export function transferSubmitDisabled(state: TransferDialogState): boolean {
  const target = state.projects.find((project) => project.path === state.targetPath);
  if (state.mode === "move") {
    return state.targetPath === "" || target === undefined || target.current;
  }
  const landsInCurrent = state.targetPath === "" || target === undefined || target.current;
  return landsInCurrent && state.title.trim() === "";
}
```

- [ ] Write `frontend/src/workspace/plan-lifecycle.test.ts` (vitest, mirror the import/describe style of `frontend/src/workspace/model.test.ts`):

```ts
import { describe, expect, it } from "vitest";

import {
  deleteConfirmationText,
  planMenuItems,
  transferSubmitDisabled,
} from "./plan-lifecycle";

describe("plan lifecycle menu", () => {
  it("offers rename, move, copy, and destructive delete", () => {
    const items = planMenuItems();
    expect(items.map((item) => item.action)).toEqual(["rename", "move", "copy", "delete"]);
    expect(items.filter((item) => item.destructive).map((item) => item.action)).toEqual(["delete"]);
  });
});

describe("delete confirmation text", () => {
  it("names counts, detached issues, and surviving commit records", () => {
    const text = deleteConfirmationText({
      planId: 3,
      title: "Doomed",
      tasks: 2,
      notes: 1,
      commits: 4,
      detachedIssues: [{ id: 7, title: "crash" }],
    });
    expect(text).toContain("2 tasks and 1 note");
    expect(text).toContain("1 linked issue will be detached");
    expect(text).toContain("4 commit records stay");
  });
});

describe("transfer submit gating", () => {
  const projects = [
    { name: "alpha", path: "/a", current: true },
    { name: "beta", path: "/b", current: false },
  ];
  it("move requires a non-current target", () => {
    expect(transferSubmitDisabled({ mode: "move", projects, targetPath: "", title: "" })).toBe(true);
    expect(transferSubmitDisabled({ mode: "move", projects, targetPath: "/a", title: "" })).toBe(true);
    expect(transferSubmitDisabled({ mode: "move", projects, targetPath: "/b", title: "" })).toBe(false);
  });
  it("copy into the current project requires a new title", () => {
    expect(transferSubmitDisabled({ mode: "copy", projects, targetPath: "", title: "" })).toBe(true);
    expect(transferSubmitDisabled({ mode: "copy", projects, targetPath: "/a", title: " " })).toBe(true);
    expect(transferSubmitDisabled({ mode: "copy", projects, targetPath: "", title: "Second" })).toBe(false);
    expect(transferSubmitDisabled({ mode: "copy", projects, targetPath: "/b", title: "" })).toBe(false);
  });
});
```

- [ ] Run to see fail then pass: `npm --prefix frontend test -- plan-lifecycle`.
- [ ] Add dialog markup to `frontend/index.html` next to the existing `#dialog-form` dialog (same overlay/dialog classes the file already uses for it):

```html
<div id="plan-dialog" class="modal" hidden>
  <form id="plan-dialog-form" class="dialog">
    <p class="dialog-eyebrow" id="plan-dialog-eyebrow"></p>
    <h2 id="plan-dialog-heading"></h2>
    <p id="plan-dialog-body"></p>
    <label id="plan-dialog-project-label" for="plan-dialog-project">Target project</label>
    <select id="plan-dialog-project"></select>
    <label id="plan-dialog-title-label" for="plan-dialog-title">New title (optional)</label>
    <input id="plan-dialog-title" type="text" autocomplete="off" />
    <p id="plan-dialog-error" class="plan-dialog-error" role="alert" hidden></p>
    <div class="dialog-actions">
      <button type="button" id="plan-dialog-cancel">Cancel</button>
      <button type="submit" id="plan-dialog-submit">OK</button>
    </div>
  </form>
</div>
```

(Match the exact class/structure conventions of the existing `#dialog-form` block in `index.html` — copy its wrapper structure, keep the new IDs.)
- [ ] Wire `frontend/src/app.js`:
  1. Import: `import { deleteConfirmationText, planMenuItems, transferSubmitDisabled } from "./workspace/plan-lifecycle";`
  2. Register the new elements in the `elements` map (`planDialog`, `planDialogForm`, `planDialogEyebrow`, `planDialogHeading`, `planDialogBody`, `planDialogProjectLabel`, `planDialogProject`, `planDialogTitleLabel`, `planDialogTitle`, `planDialogError`, `planDialogCancel`, `planDialogSubmit`).
  3. Context menu: one shared function that renders `planMenuItems()` as a positioned `<div class="context-menu" role="menu">` with one `<button role="menuitem">` per item (destructive items get class `context-menu-destructive`), dismissed on outside click, blur, and Escape. Attach it via a `contextmenu` listener AND a small `⋯` button on each sidebar plan item (in the sidebar plan renderer near the `sidebar-plan-hold` badge, app.js ~line 1500) and on the board header title element.
  4. Rename (inline, per spec): `beginPlanRename(titleElement, plan)` replaces the element's text with an `<input class="plan-rename-input">` prefilled with the current title; Enter calls `api().RenamePlanV1(ticket.generation, plan.id, input.value)` then `loadSnapshot(board?.planId || 0)`; Escape restores the original text; a failed call restores the text and shows the error via the app's existing status/toast channel (grep `setStatus` / the pattern used after failed `RenameTaskV2` calls and reuse it).
  5. Delete: `openPlanDeleteDialog(plan)` first awaits `api().DeletePlanV1(ticket.generation, plan.id, false)`, then shows `#plan-dialog` with eyebrow "Delete plan", heading `Delete “${title}”?`, body `deleteConfirmationText(response.summary)`, project/title fields hidden, submit button labeled "Delete plan" with class `dialog-danger`. Submit calls `DeletePlanV1(..., true)`, closes, `loadSnapshot(0)`. Errors (claim refusals etc.) land in `#plan-dialog-error` verbatim and keep the dialog open.
  6. Move/Copy: `openPlanTransferDialog(plan, mode)` awaits `api().ListProjectsV1(ticket.generation)`, fills the `<select>` with one `<option value="">Choose a project…</option>` plus one option per project (`${name} — ${path}`, current project suffixed " (this project)"), shows the title input, and disables submit per `transferSubmitDisabled({ mode, projects, targetPath, title })` on every `input`/`change`. Submit calls `MovePlanV1(ticket.generation, plan.id, targetPath, title)` or `CopyPlanV1(ticket.generation, plan.id, targetPath, title)` (empty strings when unset), closes on success, `loadSnapshot(0)`; errors render verbatim in `#plan-dialog-error`.
- [ ] Add styles to `frontend/src/style.css` using existing tokens (match the file's variable names — grep `--` custom properties used by `.dialog`):

```css
.context-menu {
  position: fixed;
  z-index: 60;
  min-width: 180px;
  padding: 4px;
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--surface);
  box-shadow: var(--shadow, 0 8px 24px rgba(0, 0, 0, 0.25));
}
.context-menu button {
  display: block;
  width: 100%;
  padding: 6px 10px;
  text-align: left;
  border: 0;
  background: none;
  border-radius: 6px;
}
.context-menu button:hover,
.context-menu button:focus-visible {
  background: var(--surface-raised, rgba(127, 127, 127, 0.15));
}
.context-menu-destructive,
.dialog-danger {
  color: var(--danger, #c0392b);
}
.dialog-danger {
  border-color: var(--danger, #c0392b);
}
.plan-dialog-error {
  color: var(--danger, #c0392b);
  white-space: pre-wrap;
}
.plan-rename-input {
  width: 100%;
  font: inherit;
}
```

(Adjust variable names to the ones `style.css` actually defines — grep `var(--` in the `.dialog` rules and reuse those.)
- [ ] Run to pass: `npm --prefix frontend test` and `npm --prefix frontend run build`.
- [ ] Run all quality gates (`make help-check` will FAIL here because app.js/style.css/index.html changed and the screenshot manifest hash is stale — that refresh is Task 8's first step; to keep this commit green, do the hash refresh in THIS commit instead: run the Task 8 step "refresh uiSourceSha256" now, commit it together with the frontend change).
- [ ] Commit: `git add -A && git commit -m "feat(frontend): plan context menu with rename, delete preview, and move/copy dialogs"`

---

### Task 8: Documentation — agent guide, help site, CHANGELOG, README

**Files:**
- Modify: `crates/ptrack-core/src/guide.rs` (+ its snapshot expectations in `crates/ptrack-core/src/guide_test.rs` if asserted)
- Modify: `docs/help/reference/index.html`
- Modify: `docs/help/desktop/index.html`
- Modify: `docs/help/search-index.json`
- Modify: `docs/help/assets/screenshots/manifest.json` (uiSourceSha256 refresh, if not already done in Task 7)
- Modify: `CHANGELOG.md`
- Modify: `README.md`

**Interfaces:**
- Consumes: `make help-check` contract (`tools/help_check.py`: link/route validation, `search-index.json` productVersion, screenshot-manifest `uiSourceSha256` = SHA-256 over `relative_path\0bytes` of each `uiSources` entry in order).
- Produces: user-facing docs for `plan delete|move|copy` and the GUI plan menu.

**Steps:**

- [ ] `crates/ptrack-core/src/guide.rs`: update the plan command-table row (line ~519) to
  `| \`ptrack plan add|list|show|done|use|release|rename|delete|move|copy|hold|resume\` | Manage plans; \`use <id>\` claims it (\`--steal\` to take over); \`delete <id> --force\` permanently removes a plan with its tasks and notes (issues detach, commits keep unlinked audit records); \`move <id> --to <project>\` relocates a plan subtree to another registered project (arrives unclaimed; \`--as\` renames); \`copy <id> [--to <project>] --as "<title>"\` duplicates it. |`
  and add one prose line to the plans bullet near line 19: after the existing claim sentence, `Junk plans are removed with \`ptrack plan delete <id> --force\` (preview first without \`--force\`), and work that belongs elsewhere moves with \`ptrack plan move <id> --to <project>\`.`
  Run `cargo test -p ptrack-core` and update any guide snapshot assertions the change breaks.
- [ ] `README.md` command table (line ~519): replace the plan row with
  `| \`ptrack plan add\|list\|show\|done\|use\|release\|rename\|delete\|move\|copy\|hold\|resume\` | Manage plans; \`delete <id> --force\` cascades to tasks and notes (issues detach, commit records survive unlinked); \`move <id> --to <project>\` relocates a plan subtree (copy-first, never lossy; \`--as\` renames on arrival); \`copy\` duplicates one (needs \`--as\` without \`--to\`). |`
- [ ] `CHANGELOG.md` under `## [Unreleased]`, add:

```markdown
### Added
- Plan lifecycle operations. `ptrack plan delete <id> --force` permanently
  removes a plan and cascades in one transaction: its tasks and their notes
  are deleted, linked issues survive with the task link cleared (each one is
  listed), commit records survive as audit trail with their references
  zeroed, and every active-plan pointer that named the plan resets. Without
  `--force` the command only prints what would be destroyed. `ptrack plan
  move <id> --to <project>` relocates a plan subtree to another registered
  project — the copy commits in the target before the source delete runs, so
  a crash can only ever leave a visible duplicate, never a loss; the plan
  arrives unclaimed, holds travel, and the milestone link is dropped.
  `ptrack plan copy <id> [--to <project>] --as "<title>"` duplicates a
  subtree with freshly minted IDs (`--as` required when copying within the
  same project). All three respect plan claims. The Desktop GUI gains its
  first plan-level content mutations: a plan context menu (sidebar and board
  header) with Rename (inline), Delete (with a cascade preview), and
  Move/Copy behind a project picker, over five new bridge commands
  (`RenamePlanV1`, `DeletePlanV1`, `MovePlanV1`, `CopyPlanV1`,
  `ListProjectsV1`).
```

- [ ] `docs/help/reference/index.html`: extend the existing `plan` command section (match the page's markup for `plan hold`/`plan use` entries) with `plan delete <id> --force`, `plan move <id> --to <project> [--as "<title>"]`, and `plan copy <id> [--to <project>] [--as "<title>"]`, describing the cascade (tasks and notes deleted, issues detached and listed, commit records kept unlinked), the copy-first move contract (a crash leaves a visible duplicate, never a loss), that a moved task's issue moves with it while a copied task's issue is duplicated, unclaimed arrival, hold travel, milestone drop, and the same-project rules (move refused → use rename; copy needs `--as`).
- [ ] `docs/help/desktop/index.html`: document the plan context menu (Rename inline; Delete shows exactly what will be destroyed before asking; Move/Copy open a project picker fed by the registered-projects list; store refusals — claimed plans, unopenable targets — appear verbatim in the dialog).
- [ ] `docs/help/search-index.json`: add entries for the new reference/desktop anchors following the file's existing `{title, route, ...}` row shape (copy a neighboring row and adjust; keep `productVersion` untouched).
- [ ] Refresh the screenshot manifest hash (skip if already done in Task 7's commit). From the repo root:

```bash
python3 - <<'EOF'
import hashlib, json, pathlib, re
repo = pathlib.Path(".")
manifest_path = repo / "docs/help/assets/screenshots/manifest.json"
manifest = json.loads(manifest_path.read_text())
digest = hashlib.sha256()
for relative in manifest["uiSources"]:
    digest.update(relative.encode("utf-8"))
    digest.update(b"\0")
    digest.update((repo / relative).read_bytes())
text = manifest_path.read_text()
text = re.sub(r'"uiSourceSha256": "[0-9a-f]{64}"',
              f'"uiSourceSha256": "{digest.hexdigest()}"', text)
manifest_path.write_text(text)
print(digest.hexdigest())
EOF
```

  (Regex-replace keeps the file's existing formatting byte-identical apart from the hash.)
- [ ] Run: `make help-check` until green, then ALL quality gates.
- [ ] Commit: `git add -A && git commit -m "docs: plan delete/move/copy in guide, help site, README, and changelog"`
- [ ] Final verification for the whole branch: `cargo test --workspace --all-targets --no-fail-fast && cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps && npm --prefix frontend test && make help-check`. Do NOT tag, release, or merge — finishing the branch (PR + squash) is a separate, explicitly requested step.
