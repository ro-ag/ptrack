//! Project reads: the project list, search, the activity heatmap, the
//! repository timeline, the stack profile, and the scratchpad.

use serde_json::{Value, json};

use super::super::command::{ProjectCommand, ScratchpadCommand};
use super::super::search::{heatmap, search};
use super::super::stack::{StackProfileView, StackScanOutcome, stack_scan_outcome};
use super::super::support::{lock, now_timestamp, value};
use super::{BoundDesktopWorkspace, TimelineCache};
use crate::{AppResult, ScratchpadV1};
use ptrack_core::StackProfile;
use ptrack_store::GlobalStore;

impl BoundDesktopWorkspace {
    pub(super) fn project_command(&self, command: ProjectCommand<'_>) -> AppResult<Value> {
        match command {
            ProjectCommand::ListProjects { generation } => {
                self.require_generation(generation)?;
                self.list_projects_v1()
            }
            ProjectCommand::Search { query } => value(search(&self.snapshot()?, query)),
            ProjectCommand::ActivityHeatmap { weeks } => value(heatmap(&self.snapshot()?, weeks)),
            ProjectCommand::Timeline => value(self.project_timeline_v1()),
            ProjectCommand::StackProfile { force } => value(self.stack_profile_v1(force)),
            ProjectCommand::Snapshot {
                generation,
                plan_id,
            } => {
                self.require_generation(generation)?;
                self.workspace_snapshot_v1(plan_id)
            }
        }
    }

    pub(super) fn scratchpad_command(&self, command: ScratchpadCommand) -> AppResult<Value> {
        match command {
            ScratchpadCommand::Get { generation } => {
                self.require_generation(generation)?;
                let scratchpad = ScratchpadV1::from(&lock(&self.application).scratchpad()?);
                Ok(json!({
                    "generation": self.generation,
                    "scratchpad": value(scratchpad)?,
                }))
            }
            ScratchpadCommand::Set {
                generation,
                revision,
                scratchpad,
            } => {
                self.require_exact_generation(generation)?;
                let stored =
                    lock(&self.application).set_scratchpad(revision, scratchpad.into_model())?;
                Ok(json!({
                    "generation": self.generation,
                    "revision": stored.revision,
                }))
            }
        }
    }

    fn list_projects_v1(&self) -> AppResult<Value> {
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

    /// Serves the deterministic stack profile, scanning when one is due.
    ///
    /// Every failure path degrades to a served state rather than an error: a
    /// project that cannot be scanned still opens, and the stored profile is
    /// never cleared by a failed attempt.
    fn stack_profile_v1(&self, force: bool) -> StackProfileView {
        let Ok(store) = self.project_store() else {
            return StackProfileView::unavailable();
        };
        let stored = store.stack_profile().ok().flatten();
        let head = self.repository_head();
        let outcome = stack_scan_outcome(stored, head.as_deref(), force, || {
            self.scan_stack(head.as_deref().unwrap_or_default())
        });
        match outcome {
            StackScanOutcome::Store(profile) => {
                if store.set_stack_profile(profile.clone()).is_err() {
                    return StackProfileView::failed();
                }
                self.record_registry_stack(&profile);
                StackProfileView::ready(&profile)
            }
            StackScanOutcome::Serve(profile) => StackProfileView::ready(&profile),
            StackScanOutcome::Failed(_) => StackProfileView::failed(),
            StackScanOutcome::Unavailable => StackProfileView::unavailable(),
        }
    }

    /// Repository history for the Overview, reusing the last read while HEAD
    /// has not moved.
    fn project_timeline_v1(&self) -> Value {
        let head = self.repository_head();
        if let Ok(cache) = self.timeline.lock()
            && let Some(entry) = cache.as_ref()
            && entry.head == head
        {
            return entry.value.clone();
        }
        let value = self.repository_timeline();
        if let Ok(mut cache) = self.timeline.lock() {
            *cache = Some(TimelineCache {
                head,
                value: value.clone(),
            });
        }
        value
    }

    /// Commit and tag history for the timeline, empty when the root is not a
    /// repository this build can walk.
    fn repository_timeline(&self) -> Value {
        let cancellation = ptrack_git::CancellationToken::new();
        match ptrack_git::capture_timeline(&cancellation, &self.endpoint.root) {
            Ok(timeline) => json!({
                "commits": timeline.commits,
                "tags": timeline
                    .tags
                    .iter()
                    .map(|tag| json!({ "name": tag.name, "at": tag.at }))
                    .collect::<Vec<_>>(),
                "truncated": timeline.truncated,
                "available": true,
            }),
            Err(_) => json!({
                "commits": [],
                "tags": [],
                "truncated": false,
                "available": false,
            }),
        }
    }

    /// Reads the current HEAD, or `None` when the project root is not a usable
    /// repository (no git, no commit yet).
    fn repository_head(&self) -> Option<String> {
        let cancellation = ptrack_git::CancellationToken::new();
        let identity =
            ptrack_git::inspect_worktree(&cancellation, &self.endpoint.root, &self.endpoint.root)
                .ok()?;
        (!identity.head.is_empty()).then_some(identity.head)
    }

    /// Runs one bounded tracked-path scan and resolves it.
    fn scan_stack(&self, head: &str) -> Option<StackProfile> {
        let cancellation = ptrack_git::CancellationToken::new();
        let listing = ptrack_git::RepositoryService::for_stack_scan()
            .capture_tracked_paths(&cancellation, &self.endpoint.root)
            .ok()?;
        let files: Vec<ptrack_core::stack::TrackedFile> = listing
            .paths
            .iter()
            .map(|entry| ptrack_core::stack::TrackedFile {
                path: entry.path.clone(),
                lines: entry.lines,
            })
            .collect();
        let lines = files
            .iter()
            .fold(0u32, |total, file| total.saturating_add(file.lines));
        Some(StackProfile {
            projects: ptrack_core::stack::resolve(&files),
            scanned_head: head.to_owned(),
            scanned_at: now_timestamp(),
            tracked_files: u32::try_from(files.len()).unwrap_or(u32::MAX),
            lines,
            lines_counted: listing.lines_counted,
            incomplete: listing.incomplete,
            future_fields: Vec::new(),
        })
    }

    /// Mirrors the profile's summary onto the global registry entry so the
    /// project cards can label a project without opening its database.
    fn record_registry_stack(&self, profile: &StackProfile) {
        let Ok(store) = GlobalStore::open_existing(
            &self.bindings.global_database,
            &self.bindings.global_binding,
        ) else {
            return;
        };
        let _ =
            store.set_project_stack(&self.endpoint.root, ptrack_core::stack::summarize(profile));
    }
}
