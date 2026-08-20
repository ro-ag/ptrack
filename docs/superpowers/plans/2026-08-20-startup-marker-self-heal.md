# Startup Marker Self-Heal Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Startup silently prunes marker projects whose root directories were deleted, instead of refusing to start.

**Architecture:** `ActiveRuntime::load` (crates/ptrack-app/src/production.rs) becomes attempt → on error, prune-missing-roots under the exclusive cutover lock → retry once. Pruning backs up the old marker and republishes through the existing `install_active_generation`, which re-validates the remainder. All other failures stay fail-closed.

**Tech Stack:** Rust, existing ptrack-store cutover/marker primitives. No new dependencies.

## Global Constraints

- Heal only on direct evidence: a listed project root whose directory is absent (`fs::symlink_metadata` probe, no symlink following). Marker corruption, non-canonical paths, writer-version mismatches, global-store failures: unchanged fail-closed errors.
- Prune errors must never mask the original load error — fall back to returning it.
- Backup file `runtime/active-generation.json.pruned-<unix-epoch>` must be private (0600 via `protect_private_file`).
- Spec: docs/superpowers/specs/2026-08-20-startup-marker-self-heal-design.md.
- Repo rules: no AI attribution in commits/PRs; branch `feat/startup-marker-self-heal` already exists; land via PR + squash merge.
- Known out-of-scope: `RoutedApplication::bootstrap` validates the existing marker under its own exclusive lease and still fails closed if `ptrack init` is the first command after a root deletion (any data command or GUI start heals first; flock non-reentrancy prevents nesting the prune there).

---

### Task 1: Self-heal in `ActiveRuntime::load`

**Files:**
- Modify: `crates/ptrack-app/src/production.rs:174-200` (`ActiveRuntime::load`) plus new free functions near `fn recovery` (~line 2776)
- Test: `crates/ptrack-app/src/production_test.rs`

**Interfaces:**
- Consumes: `acquire_cutover_lock`, `load_active_generation`, `install_active_generation`, `protect_private_file`, `path_is_present`, `recovery` — all already imported/defined in production.rs. `ActiveGeneration`/`ActiveGenerationProject` are pub-field Clone structs (ptrack-store/src/runtime_binding.rs:27,44).
- Produces: unchanged public signature `ActiveRuntime::load(global_home, writer_version) -> AppResult<Option<Arc<Self>>>`; private `fn prune_missing_marker_projects(&Path, &str) -> AppResult<bool>`, `fn backup_marker(&Path) -> AppResult<()>`, private assoc `fn attempt(&Path, &str) -> AppResult<Option<Arc<ActiveRuntime>>>`.

- [ ] **Step 1: Write the failing tests**

Append to `crates/ptrack-app/src/production_test.rs` (uses existing `Temp`, `private_directory`, `private_file` helpers; `ActiveRuntime`, `RoutedApplication`, `InitRequest`, `acquire_cutover_lock`, `CutoverLockMode` already imported):

```rust
#[test]
fn load_self_heals_marker_projects_with_missing_roots() {
    let temp = Temp::new();
    let home = temp.0.join("home");
    let kept = temp.0.join("kept");
    let doomed = temp.0.join("doomed");
    fs::create_dir(&home).unwrap();
    fs::create_dir(&kept).unwrap();
    fs::create_dir(&doomed).unwrap();
    private_directory(&home);

    let mut application = RoutedApplication::new(home.clone(), kept.clone(), "test");
    application
        .initialize(InitRequest {
            root: Some(kept.clone()),
            goal: String::new(),
            force: false,
            no_guide: true,
        })
        .unwrap();
    drop(application);
    let mut application = RoutedApplication::new(home.clone(), doomed.clone(), "test");
    application
        .initialize(InitRequest {
            root: Some(doomed.clone()),
            goal: String::new(),
            force: false,
            no_guide: true,
        })
        .unwrap();
    drop(application);

    fs::remove_dir_all(&doomed).unwrap();

    let runtime = ActiveRuntime::load(&home, "test").unwrap().unwrap();
    assert_eq!(runtime.marker().projects.len(), 1);
    assert_eq!(runtime.marker().projects[0].root, kept.to_string_lossy());
    let backups = fs::read_dir(home.join("runtime"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("active-generation.json.pruned-")
        })
        .count();
    assert_eq!(backups, 1);
}

#[test]
fn load_still_fails_closed_for_a_noncanonical_marker() {
    let temp = Temp::new();
    let home = temp.0.join("home");
    let project = temp.0.join("project");
    fs::create_dir(&home).unwrap();
    fs::create_dir(&project).unwrap();
    private_directory(&home);
    let mut application = RoutedApplication::new(home.clone(), project.clone(), "test");
    application
        .initialize(InitRequest {
            root: Some(project.clone()),
            goal: String::new(),
            force: false,
            no_guide: true,
        })
        .unwrap();
    drop(application);

    let marker_path = home.join("runtime/active-generation.json");
    let lease = acquire_cutover_lock(&home, CutoverLockMode::Exclusive).unwrap();
    let mut tampered = b" ".to_vec();
    tampered.extend_from_slice(&fs::read(&marker_path).unwrap());
    fs::write(&marker_path, &tampered).unwrap();
    private_file(&marker_path);
    drop(lease);

    let error = ActiveRuntime::load(&home, "test").unwrap_err().to_string();
    assert!(
        error.contains("runtime recovery is required"),
        "error: {error}"
    );
    assert_eq!(fs::read(&marker_path).unwrap(), tampered);
}

#[test]
fn load_does_not_rewrite_a_marker_whose_roots_all_exist() {
    let temp = Temp::new();
    let home = temp.0.join("home");
    let project = temp.0.join("project");
    fs::create_dir(&home).unwrap();
    fs::create_dir(&project).unwrap();
    private_directory(&home);
    let mut application = RoutedApplication::new(home.clone(), project.clone(), "test");
    application
        .initialize(InitRequest {
            root: Some(project.clone()),
            goal: String::new(),
            force: false,
            no_guide: true,
        })
        .unwrap();
    drop(application);

    let marker_path = home.join("runtime/active-generation.json");
    let before = fs::read(&marker_path).unwrap();
    let runtime = ActiveRuntime::load(&home, "test").unwrap().unwrap();
    assert_eq!(runtime.marker().projects.len(), 1);
    drop(runtime);
    assert_eq!(fs::read(&marker_path).unwrap(), before);
    let backups = fs::read_dir(home.join("runtime"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .contains(".pruned-")
        })
        .count();
    assert_eq!(backups, 0);
}
```

- [ ] **Step 2: Run tests to verify the heal test fails**

Run: `cargo test -p ptrack-app load_self_heals -- --nocapture` and `cargo test -p ptrack-app load_still_fails_closed load_does_not_rewrite`
Expected: `load_self_heals_marker_projects_with_missing_roots` FAILS (load returns the recovery error). The two fail-closed tests PASS already (they pin current behavior).

- [ ] **Step 3: Implement the heal**

In `crates/ptrack-app/src/production.rs`, replace the body of `ActiveRuntime::load` (keep its doc comment, extend `# Errors` to mention the retry) and add the private helpers:

```rust
    /// Loads and attests the sole active-generation marker.
    ///
    /// When validation fails because listed project roots were deleted, the
    /// missing projects are pruned from the marker under the exclusive
    /// cutover lock (the replaced marker is backed up beside it) and the
    /// load is retried once.
    ///
    /// # Errors
    /// Returns a recovery-required error for an unsafe marker, lock, or store.
    pub fn load(
        global_home: impl AsRef<Path>,
        writer_version: impl Into<String>,
    ) -> AppResult<Option<Arc<Self>>> {
        let writer_version = writer_version.into();
        let global_home = global_home.as_ref();
        match Self::attempt(global_home, &writer_version) {
            Err(error) => {
                if prune_missing_marker_projects(global_home, &writer_version).unwrap_or(false) {
                    Self::attempt(global_home, &writer_version)
                } else {
                    Err(error)
                }
            }
            loaded => loaded,
        }
    }

    fn attempt(global_home: &Path, writer_version: &str) -> AppResult<Option<Arc<Self>>> {
        if !global_home.exists() {
            return Ok(None);
        }
        let home = fs::canonicalize(global_home).map_err(recovery)?;
        let lease = acquire_cutover_lock(&home, CutoverLockMode::Shared).map_err(recovery)?;
        if path_is_present(&home.join("runtime").join(BOOTSTRAP_PLAN))? {
            return Err(recovery(
                "bootstrap recovery must complete before runtime load",
            ));
        }
        let Some(marker) = load_active_generation(&home, &lease).map_err(recovery)? else {
            return Ok(None);
        };
        validate_active_generation(&home, &marker, writer_version).map_err(recovery)?;
        Ok(Some(Arc::new(Self {
            home,
            marker,
            writer_version: writer_version.to_owned(),
            _lease: lease,
        })))
    }
```

Free functions (place next to `fn recovery`, ~line 2776). `ActiveGenerationProject` must be added to the `ptrack_store` import list at the top of production.rs (`ActiveGeneration` is already there):

```rust
/// Prunes marker projects whose root directories no longer exist so one
/// deleted project cannot block every startup. Returns true only when a
/// pruned marker was published; any other outcome leaves the caller's
/// original fail-closed error in force. The exclusive lock is non-blocking,
/// so a live process holding the shared lease makes this return an error
/// rather than wait.
fn prune_missing_marker_projects(global_home: &Path, writer_version: &str) -> AppResult<bool> {
    if !global_home.exists() {
        return Ok(false);
    }
    let home = fs::canonicalize(global_home).map_err(recovery)?;
    if path_is_present(&home.join("runtime").join(BOOTSTRAP_PLAN))? {
        return Ok(false);
    }
    let lease = acquire_cutover_lock(&home, CutoverLockMode::Exclusive).map_err(recovery)?;
    let Some(marker) = load_active_generation(&home, &lease).map_err(recovery)? else {
        return Ok(false);
    };
    let kept: Vec<ActiveGenerationProject> = marker
        .projects
        .iter()
        .filter(|project| path_is_present(Path::new(&project.root)).unwrap_or(true))
        .cloned()
        .collect();
    if kept.len() == marker.projects.len() {
        return Ok(false);
    }
    backup_marker(&home)?;
    let pruned = ActiveGeneration {
        projects: kept,
        ..marker
    };
    install_active_generation(&home, &lease, &pruned, writer_version).map_err(recovery)?;
    Ok(true)
}

fn backup_marker(home: &Path) -> AppResult<()> {
    let marker = home.join("runtime").join("active-generation.json");
    let backup = home.join("runtime").join(format!(
        "active-generation.json.pruned-{}",
        OffsetDateTime::now_utc().unix_timestamp()
    ));
    fs::copy(&marker, &backup)?;
    protect_private_file(&backup).map_err(recovery)?;
    Ok(())
}
```

Notes for the implementer:
- `path_is_present(...).unwrap_or(true)` is deliberate: a probe error keeps the project (fail-closed), and the subsequent no-op prune returns the original error to the user.
- `install_active_generation` re-validates the pruned marker (including writer-version attestation of every kept store) before publishing; a still-broken marker is therefore never written.
- `fs::copy` preserves the 0600 mode on unix; `protect_private_file` attests it either way.

- [ ] **Step 4: Run the tests and full crate suite**

Run: `cargo test -p ptrack-app`
Expected: all three new tests PASS, no existing test regresses.

- [ ] **Step 5: Format and lint**

Run: `cargo fmt --all && cargo clippy -p ptrack-app --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/ptrack-app/src/production.rs crates/ptrack-app/src/production_test.rs
git commit -m "feat(app): self-heal active-generation marker at startup"
```

---

### Task 2: Land the branch

**Files:** none new.

**Interfaces:** n/a.

- [ ] **Step 1: Workspace gates**

Run: `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings`
Expected: green. (Add `docs/superpowers/plans/2026-08-20-startup-marker-self-heal.md` in a `docs:` commit if not yet committed.)

- [ ] **Step 2: PR + squash merge**

```bash
git push -u origin feat/startup-marker-self-heal
gh pr create --title "feat(app): self-heal active-generation marker at startup" --body "Startup prunes marker projects whose root directories were deleted (backing up the replaced marker) instead of failing closed on the whole app. Spec: docs/superpowers/specs/2026-08-20-startup-marker-self-heal-design.md"
gh pr merge --squash --delete-branch
git checkout main && git pull
```

No AI attribution anywhere.
