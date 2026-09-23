//! The initialization commit: one exclusive-lease transaction that publishes
//! the bootstrap plan, installs the marker, commits the project store, and
//! applies the guide decision, recording each checkpoint in order so an
//! interruption resumes exactly where it stopped.

use std::fs;
use std::path::{Path, PathBuf};

use ptrack_store::{
    ActiveGeneration, CutoverLease, CutoverLockMode, GlobalStore, PinnedProjectDirectory,
    PrivatePathIdentity, ProjectStore, StoreKind, acquire_cutover_lock, install_active_generation,
    load_active_generation, validate_active_generation,
};

use super::super::bootstrap::{
    BootstrapPlan, DirectoryIdentity, binding_for_new, clear_bootstrap_plan, directory_identity,
    ensure_bootstrap_stores, ensure_private_home, new_bootstrap_plan, publish_bootstrap_plan,
    read_bootstrap_plan, require_directory_identity, require_new_project_storage_absent,
    validate_bootstrap_plan,
};
use super::super::guide::{
    DesktopGuideManifest, apply_guide_manifest, validate_guide_before_commit,
};
use super::super::journal::read_desktop_initialization;
use super::super::{BOOTSTRAP_PLAN, lock, path_is_present, recovery};
use super::ProductionDesktopAuthority;
use crate::{
    AppResult, InitializationCheckpointV1, InitializationOutcomeV1, InitializationStatusV1,
    InitializeProjectRequestV1,
};

impl ProductionDesktopAuthority {
    /// Commits one readied initialization from its recorded checkpoint.
    pub(super) fn commit_initialization(
        &self,
        request: &InitializeProjectRequestV1,
        ready: &InitializationStatusV1,
    ) -> AppResult<InitializationStatusV1> {
        let guide = lock(&self.state)
            .initialization_guide
            .clone()
            .ok_or_else(|| recovery("project guide choice is not bound to initialization"))?;
        let root = PathBuf::from(&ready.canonical_root);
        let root_identity = directory_identity(&root)?;
        let pinned_root_identity =
            PinnedProjectDirectory::identify_root(&root).map_err(recovery)?;
        if ready.checkpoint == InitializationCheckpointV1::None {
            require_new_project_storage_absent(&root, &self.global_home)?;
        }
        ensure_private_home(&self.global_home)?;
        let home = fs::canonicalize(&self.global_home)?;
        // The lease modes already serialize this against a command-line
        // initialization: that one appends under the shared lease and
        // re-checks the marker before it publishes, and this exclusive lease
        // cannot coexist with it. Taking the initializers' bootstrap lock here
        // as well only let a refused initialization reload its authority —
        // reclaiming the shared lease — before this one reached the lease it
        // needs, so both raced to failure.
        let lease = acquire_cutover_lock(&home, CutoverLockMode::Exclusive).map_err(recovery)?;
        let commit = Commit {
            authority: self,
            request,
            ready,
            guide,
            plan_path: home.join("runtime").join(BOOTSTRAP_PLAN),
            root,
            root_identity,
            home,
        };
        let existing = commit.locked_marker(&lease)?;
        commit.record(ready.checkpoint)?;
        #[cfg(test)]
        super::super::test_support::run_initialization_after_started_hook();
        let (existing_plan, pinned_project) = commit.pinned_project(pinned_root_identity)?;
        match ready.checkpoint {
            InitializationCheckpointV1::GuideApplied => commit.finish_guide_applied(
                existing.as_ref(),
                existing_plan.as_ref(),
                &pinned_project,
            ),
            InitializationCheckpointV1::ProjectCommitted => commit.resume_project_committed(
                existing.as_ref(),
                existing_plan.as_ref(),
                &pinned_project,
            ),
            _ => {
                let plan =
                    commit.bootstrap_plan(existing.as_ref(), existing_plan, &pinned_project)?;
                let marker =
                    commit.install_marker(&lease, existing, plan.as_ref(), &pinned_project)?;
                commit.commit_project(&marker, plan.is_some(), &pinned_project)
            }
        }
    }
}

/// One initialization commit and everything it resolved before taking the
/// exclusive lease.
struct Commit<'a> {
    authority: &'a ProductionDesktopAuthority,
    request: &'a InitializeProjectRequestV1,
    ready: &'a InitializationStatusV1,
    guide: DesktopGuideManifest,
    root: PathBuf,
    root_identity: DirectoryIdentity,
    home: PathBuf,
    plan_path: PathBuf,
}

impl Commit<'_> {
    /// This operation's in-progress status at `checkpoint`.
    fn in_progress(&self, checkpoint: InitializationCheckpointV1) -> InitializationStatusV1 {
        InitializationStatusV1 {
            operation_id: self.request.operation_id.clone(),
            canonical_root: self.ready.canonical_root.clone(),
            checkpoint,
            outcome: InitializationOutcomeV1::InProgress,
            error_kind: String::new(),
        }
    }

    /// Durably records this operation as in progress at `checkpoint`.
    fn record(&self, checkpoint: InitializationCheckpointV1) -> AppResult<InitializationStatusV1> {
        self.authority
            .record_initialization_status(self.in_progress(checkpoint), &self.request.goal)
    }

    /// Re-reads the journal and the marker under the exclusive lease and
    /// refuses anything that changed since the target was classified.
    fn locked_marker(&self, lease: &CutoverLease) -> AppResult<Option<ActiveGeneration>> {
        let (request, ready) = (self.request, self.ready);
        if let Some(journal) = read_desktop_initialization(&self.home)? {
            if journal.status.outcome != InitializationOutcomeV1::Complete
                && journal.status.operation_id != request.operation_id
                && journal.status.checkpoint != InitializationCheckpointV1::None
            {
                return Err(recovery("another initialization operation is incomplete"));
            }
            if journal.status.operation_id == request.operation_id && journal.goal != request.goal {
                return Err(recovery(
                    "initialization goal does not match the durable operation",
                ));
            }
        }
        let existing = load_active_generation(&self.home, lease).map_err(recovery)?;
        if let Some(marker) = &existing {
            validate_active_generation(&self.home, marker, &self.authority.writer_version)
                .map_err(recovery)?;
        }
        if ready.checkpoint == InitializationCheckpointV1::None {
            if existing.as_ref().is_some_and(|marker| {
                marker
                    .projects
                    .iter()
                    .any(|project| self.root.starts_with(Path::new(&project.root)))
            }) {
                return Err(recovery("selected project root is already registered"));
            }
            require_new_project_storage_absent(&self.root, &self.authority.global_home)?;
            if existing.is_none() && path_is_present(&self.home.join("global.redb"))? {
                return Err(recovery("global runtime state changed before commit"));
            }
        }
        if ready.checkpoint != InitializationCheckpointV1::GuideApplied {
            #[cfg(test)]
            super::super::test_support::run_guide_before_commit_hook();
            validate_guide_before_commit(&self.home, &self.guide)?;
        }
        Ok(existing)
    }

    /// The bootstrap plan to resume, if one is durable, and the project
    /// directory pinned to the identity this operation expects.
    fn pinned_project(
        &self,
        pinned_root_identity: PrivatePathIdentity,
    ) -> AppResult<(Option<BootstrapPlan>, PinnedProjectDirectory)> {
        let (request, ready, root) = (self.request, self.ready, &self.root);
        Ok(if path_is_present(&self.plan_path)? {
            let plan = read_bootstrap_plan(&self.plan_path)?;
            validate_bootstrap_plan(&self.home, root, &plan, &self.authority.writer_version)?;
            if plan.operation_id.as_deref() != Some(request.operation_id.as_str()) {
                return Err(recovery(
                    "bootstrap plan is not bound to this initialization operation",
                ));
            }
            let pinned = PinnedProjectDirectory::prepare_expected_identities(
                root,
                plan.project_root_identity,
                plan.project_directory_identity,
            )
            .map_err(recovery)?;
            (Some(plan), pinned)
        } else if matches!(
            ready.checkpoint,
            InitializationCheckpointV1::RuntimeCommitted
                | InitializationCheckpointV1::ProjectCommitted
                | InitializationCheckpointV1::GuideApplied
        ) {
            let expected_root = self
                .guide
                .root_identity
                .ok_or_else(|| recovery("project guide root identity is missing"))?;
            (
                None,
                PinnedProjectDirectory::prepare_expected(root, expected_root).map_err(recovery)?,
            )
        } else {
            (
                None,
                PinnedProjectDirectory::prepare_new_expected(root, pinned_root_identity)
                    .map_err(recovery)?,
            )
        })
    }

    /// Resumes an operation whose guide was already applied: the commit only
    /// has to prove the marker and project are still the ones it published.
    fn finish_guide_applied(
        &self,
        existing: Option<&ActiveGeneration>,
        existing_plan: Option<&BootstrapPlan>,
        pinned_project: &PinnedProjectDirectory,
    ) -> AppResult<InitializationStatusV1> {
        let marker =
            existing.ok_or_else(|| recovery("committed initialization marker is missing"))?;
        if existing_plan.is_some_and(|plan| plan.target_marker != *marker) {
            return Err(recovery(
                "bootstrap plan does not match the committed marker",
            ));
        }
        pinned_project.verify().map_err(recovery)?;
        if existing_plan.is_some() {
            clear_bootstrap_plan(&self.plan_path)?;
        }
        Ok(self.ready.clone())
    }

    /// Resumes an operation whose project store committed: it re-proves the
    /// committed goal and then applies the guide decision.
    fn resume_project_committed(
        &self,
        existing: Option<&ActiveGeneration>,
        existing_plan: Option<&BootstrapPlan>,
        pinned_project: &PinnedProjectDirectory,
    ) -> AppResult<InitializationStatusV1> {
        let (request, ready, root) = (self.request, self.ready, &self.root);
        let marker =
            existing.ok_or_else(|| recovery("committed initialization marker is missing"))?;
        if existing_plan.is_some_and(|plan| plan.target_marker != *marker) {
            return Err(recovery(
                "bootstrap plan does not match the committed marker",
            ));
        }
        let generation = marker.generation_number()?;
        let project = marker
            .projects
            .iter()
            .find(|project| project.root == ready.canonical_root)
            .ok_or_else(|| recovery("committed initialization project is missing"))?;
        let binding = binding_for_new(
            generation,
            project.database_id.clone(),
            StoreKind::Project,
            Path::new(&project.path),
        )?;
        require_directory_identity(root, self.root_identity)?;
        pinned_project.verify().map_err(recovery)?;
        let committed_store = ProjectStore::open_existing_pinned(
            pinned_project,
            &binding,
            &self.authority.writer_version,
        )
        .map_err(recovery)?;
        let committed_goal = committed_store.meta().map_err(recovery)?.goal;
        drop(committed_store);
        pinned_project.verify().map_err(recovery)?;
        if committed_goal != request.goal {
            return Err(recovery("committed initialization goal changed"));
        }
        if existing_plan.is_some() {
            clear_bootstrap_plan(&self.plan_path)?;
        }
        apply_guide_manifest(&self.home, &self.guide, pinned_project)?;
        self.record(InitializationCheckpointV1::GuideApplied)
    }

    /// Selects the bootstrap plan this commit publishes — the durable one, a
    /// fresh one, or none when the marker already maps the root — and records
    /// the prepared checkpoint when there is one.
    fn bootstrap_plan(
        &self,
        existing: Option<&ActiveGeneration>,
        existing_plan: Option<BootstrapPlan>,
        pinned_project: &PinnedProjectDirectory,
    ) -> AppResult<Option<BootstrapPlan>> {
        let (request, ready, root) = (self.request, self.ready, &self.root);
        let plan = if let Some(plan) = existing_plan {
            Some(plan)
        } else if existing.is_some_and(|marker| {
            marker
                .projects
                .iter()
                .any(|project| project.root == ready.canonical_root)
        }) && ready.checkpoint != InitializationCheckpointV1::None
        {
            None
        } else {
            require_directory_identity(root, self.root_identity)?;
            let plan = new_bootstrap_plan(
                &self.home,
                root,
                pinned_project.root_identity(),
                pinned_project.directory_identity(),
                existing.cloned(),
                Some(request.operation_id.clone()),
            )?;
            publish_bootstrap_plan(&self.plan_path, &plan)?;
            #[cfg(test)]
            super::super::test_support::run_initialization_after_bootstrap_plan_hook();
            Some(plan)
        };
        if plan.is_some() {
            self.record(InitializationCheckpointV1::Prepared)?;
        }
        Ok(plan)
    }

    /// Installs the plan's target marker, or keeps the committed one.
    fn install_marker(
        &self,
        lease: &CutoverLease,
        existing: Option<ActiveGeneration>,
        plan: Option<&BootstrapPlan>,
        pinned_project: &PinnedProjectDirectory,
    ) -> AppResult<ActiveGeneration> {
        let (home, root) = (&self.home, &self.root);
        let writer_version = &self.authority.writer_version;
        Ok(if let Some(plan) = plan {
            if existing.as_ref() != Some(&plan.target_marker) {
                if existing != plan.previous_marker {
                    return Err(recovery(
                        "bootstrap plan does not match the active-generation marker",
                    ));
                }
                require_directory_identity(root, self.root_identity)?;
                ensure_bootstrap_stores(home, plan, writer_version, Some(pinned_project))?;
                install_active_generation(home, lease, &plan.target_marker, writer_version)
                    .map_err(recovery)?;
                pinned_project.verify().map_err(recovery)?;
            }
            plan.target_marker.clone()
        } else {
            existing.ok_or_else(|| recovery("committed initialization marker is missing"))?
        })
    }

    /// Commits the project store's goal under the installed marker, registers
    /// the project, and applies the guide decision.
    fn commit_project(
        &self,
        marker: &ActiveGeneration,
        published_plan: bool,
        pinned_project: &PinnedProjectDirectory,
    ) -> AppResult<InitializationStatusV1> {
        let (request, ready, root) = (self.request, self.ready, &self.root);
        self.record(InitializationCheckpointV1::RuntimeCommitted)?;
        let generation = marker.generation_number()?;
        let project = marker
            .projects
            .iter()
            .find(|project| project.root == ready.canonical_root)
            .ok_or_else(|| recovery("committed initialization project is missing"))?;
        let project_binding = binding_for_new(
            generation,
            project.database_id.clone(),
            StoreKind::Project,
            Path::new(&project.path),
        )?;
        require_directory_identity(root, self.root_identity)?;
        pinned_project.verify().map_err(recovery)?;
        let project_store = ProjectStore::open_existing_pinned(
            pinned_project,
            &project_binding,
            &self.authority.writer_version,
        )
        .map_err(recovery)?;
        project_store
            .set_goal(request.goal.clone())
            .map_err(recovery)?;
        drop(project_store);
        pinned_project.verify().map_err(recovery)?;

        let global_binding = binding_for_new(
            generation,
            marker.global.database_id.clone(),
            StoreKind::Global,
            Path::new(&marker.global.path),
        )?;
        if let Ok(global_store) = GlobalStore::open_existing(&marker.global.path, &global_binding) {
            let name = root
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            let _ = global_store.register_project(name, root);
        }
        if published_plan {
            clear_bootstrap_plan(&self.plan_path)?;
        }
        self.record(InitializationCheckpointV1::ProjectCommitted)?;
        apply_guide_manifest(&self.home, &self.guide, pinned_project)?;
        self.record(InitializationCheckpointV1::GuideApplied)
    }
}
