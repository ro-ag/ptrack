//! Application-scoped commands: preferences, layout, diagnostics, the
//! application-state reset, popped-out terminal windows, and the
//! recent-projects registry. Every one stays reachable on Welcome.

use std::path::Path;
use std::sync::Arc;

use ptrack_store::GlobalStore;
use serde_json::{Value, json};

use super::super::command::{
    RecentCommand, RecentOpenTarget, SettingsCommand, TerminalWindowCommand,
};
use super::super::support::{lock, value};
use super::super::wire::{
    ActiveResourceSummary, OpenRecentProjectResultV1, RecentProjectOpenAuthorizationV1,
    RecentProjectRegistryCommitV1, RecentProjectRegistryStatusV1, ResetApplicationStateResultV1,
    WorkspaceChangeResult, WorkspaceStatus,
};
use super::{CachedGlobalBinding, DesktopRuntime, marker_stamp};
use crate::diagnostics_report::{CapabilityCountsV1, DiagnosticsReportV1};
use crate::layout_state::{layout_state, reset_window_layout, set_layout_state};
use crate::preferences::{PreferencesDocumentV1, preferences, reset_preferences, set_preferences};
use crate::{ActiveRuntime, AppError, AppResult};

impl DesktopRuntime {
    pub(super) fn settings_command(
        self: &Arc<Self>,
        command: SettingsCommand<'_>,
    ) -> AppResult<Value> {
        let _lease = self.begin_native_action()?;
        match command {
            SettingsCommand::GetPreferences => value(preferences(&self.global_store()?)),
            SettingsCommand::SetPreferences { patch } => value(apply_preferences(
                &self.global_store()?,
                patch,
                self.open_project_root().as_deref(),
            )?),
            SettingsCommand::ResetPreferences => value(reset_preferences(&self.global_store()?)?),
            SettingsCommand::GetDiagnosticsReport => value(self.diagnostics_report()?),
            SettingsCommand::GetLayoutState => value(layout_state(&self.global_store()?)),
            SettingsCommand::SetLayoutState { state } => {
                value(set_layout_state(&self.global_store()?, state)?)
            }
            SettingsCommand::ResetWindowLayout => {
                value(reset_window_layout(&self.global_store()?)?)
            }
            SettingsCommand::ResetApplicationState => self.reset_application_state(),
        }
    }

    pub(super) fn terminal_window_command(
        &self,
        command: TerminalWindowCommand<'_>,
    ) -> AppResult<Value> {
        match command {
            TerminalWindowCommand::Open { tab } => {
                let opened = self.open_terminal_window(tab)?;
                lock(&self.expired_terminal_windows).extend(opened.expired);
                Ok(json!({ "label": opened.label }))
            }
            TerminalWindowCommand::GetTab { label } => Ok(match self.terminal_window_tab(label) {
                Some(tab) => json!({ "sessions": tab.sessions, "shape": tab.shape }),
                None => json!({ "sessions": null, "shape": null }),
            }),
            TerminalWindowCommand::SetTab { label, tab } => {
                self.set_terminal_window_tab(label, tab)?;
                Ok(json!({}))
            }
        }
    }

    /// Clears every app-scoped record and revokes every capability grant, and
    /// reports what went so the confirmation dialog can be honest. Grants live
    /// in the project, so revoking them writes to the open project's store
    /// through the store itself; capability brokering is retired and no broker
    /// exists to ask. Plans, tasks, notes, the recents registry, and capability
    /// definitions are untouched. The store is opened and the records are
    /// deleted first: a store that cannot be opened must not cost the user
    /// their grants for nothing.
    fn reset_application_state(self: &Arc<Self>) -> AppResult<Value> {
        let records = reset_application_records(&self.global_store()?)?;
        value(ResetApplicationStateResultV1 {
            records,
            capability_grants: self.revoke_capability_grants()?,
        })
    }

    /// Revokes every leftover capability grant in the open workspace, which
    /// keeps the operator's capability definitions. There is nothing to revoke
    /// while no workspace is open.
    fn revoke_capability_grants(&self) -> AppResult<usize> {
        if self.workspace_state().status != WorkspaceStatus::Open {
            return Ok(0);
        }
        self.with_open_workspace(|workspace| workspace.revoke_capability_grants())
    }

    /// Opens the global store for project-independent application state. The
    /// home is the same fixed platform home the host resolved at startup.
    /// Opens the global store for one application-state command.
    ///
    /// Loading the runtime attests every registered project, far too much for
    /// the layout writes a window resize sends, so the attested global binding
    /// is cached and reused while the generation marker is unchanged. Only the
    /// binding is kept, never the runtime: its shared cutover lease would
    /// block the exclusive lease a desktop initialization needs.
    pub(super) fn global_store(&self) -> AppResult<GlobalStore> {
        let home = crate::resolve_global_home()?;
        let stamp = marker_stamp(&home);
        let cached = lock(&self.global_binding)
            .clone()
            .filter(|cached| cached.home == home && stamp.is_some() && cached.stamp == stamp);
        if let Some(cached) = cached
            && let Ok(store) = GlobalStore::open_existing(&cached.database, &cached.binding)
        {
            return Ok(store);
        }
        let runtime = ActiveRuntime::load(&home, &self.version)?.ok_or_else(|| {
            AppError::Message("p-track runtime is not initialized (run 'ptrack init')".to_owned())
        })?;
        let bindings = runtime.global_bindings(runtime.global_home())?;
        let store =
            GlobalStore::open_existing(&bindings.global_database, &bindings.global_binding)?;
        *lock(&self.global_binding) = Some(CachedGlobalBinding {
            home,
            stamp,
            database: bindings.global_database,
            binding: bindings.global_binding,
        });
        Ok(store)
    }

    fn diagnostics_report(&self) -> AppResult<DiagnosticsReportV1> {
        let home = crate::resolve_global_home()?;
        let state = self.workspace_state();
        Ok(crate::diagnostics_report::report(
            &home,
            &self.version,
            state.project.as_ref(),
            self.capability_counts(),
        ))
    }

    /// Counts leftover capability grants through the open workspace. Absent
    /// while no project workspace can answer for them.
    fn capability_counts(&self) -> Option<CapabilityCountsV1> {
        self.with_open_workspace(|workspace| Ok(workspace.capability_counts()))
            .ok()
            .flatten()
    }

    pub(super) fn recent_command(self: &Arc<Self>, command: RecentCommand<'_>) -> AppResult<Value> {
        let _lease = self.begin_native_action()?;
        match command {
            RecentCommand::GlobalOverview => value(self.recent_projects.global_overview_v1()?),
            RecentCommand::RefreshGlobalOverview => {
                let _mutation = lock(&self.recent_mutation);
                value(self.recent_projects.refresh_global_overview_v1()?)
            }
            RecentCommand::List => value(self.recent_projects.recent_projects_v1()?),
            RecentCommand::Resolve {
                entry_id,
                base,
                candidate,
            } => value(
                self.recent_projects
                    .resolve_recent_project(entry_id, base, &candidate)?,
            ),
            RecentCommand::Forget { entry_id, base } => {
                let _mutation = lock(&self.recent_mutation);
                value(self.recent_projects.forget_recent_project(entry_id, base)?)
            }
            RecentCommand::Open {
                workspace_token,
                target,
            } => {
                let _mutation = lock(&self.recent_mutation);
                self.open_recent_project(workspace_token, target)
            }
        }
    }

    fn open_recent_project(
        self: &Arc<Self>,
        workspace_token: &str,
        target: AppResult<RecentOpenTarget<'_>>,
    ) -> AppResult<Value> {
        let authorization = target.and_then(|target| {
            self.recent_projects.authorize_recent_project_open(
                target.entry_id,
                target.base,
                &target.canonical_root,
                target.relocation_token,
            )
        });
        let authorization = match authorization {
            Ok(authorization) => authorization,
            Err(error) => {
                if !workspace_token.is_empty() {
                    self.cancel_workspace_change_if_exact(workspace_token);
                }
                return Err(error);
            }
        };
        let mut open = if authorization.already_completed
            && let Some(completed) = self.completed_recent_open(&authorization)
        {
            completed
        } else {
            match self.open_project(Path::new(&authorization.canonical_root), workspace_token) {
                Ok(open) => open,
                Err(error) => {
                    if !workspace_token.is_empty() {
                        self.cancel_workspace_change_if_exact(workspace_token);
                    }
                    return Err(sanitize_recent_open_error(error));
                }
            }
        };
        let commit = if open.requires_confirmation {
            RecentProjectRegistryCommitV1 {
                base: authorization.base.clone(),
                status: RecentProjectRegistryStatusV1::Unchanged,
            }
        } else {
            self.recent_projects
                .finish_recent_project_open(&authorization)
                .unwrap_or_else(|_| {
                    if open.warning.is_empty() {
                        "recent-project registry update is incomplete"
                            .clone_into(&mut open.warning);
                    }
                    RecentProjectRegistryCommitV1 {
                        base: authorization.base.clone(),
                        status: RecentProjectRegistryStatusV1::Stale,
                    }
                })
        };
        value(OpenRecentProjectResultV1 {
            open,
            entry_id: authorization.entry_id,
            registry_base: commit.base,
            registry_status: commit.status,
        })
    }

    fn completed_recent_open(
        &self,
        authorization: &RecentProjectOpenAuthorizationV1,
    ) -> Option<WorkspaceChangeResult> {
        let state = self.workspace_state();
        if state.status != WorkspaceStatus::Open
            || state.project.as_ref().map(|project| project.root.as_str())
                != Some(authorization.canonical_root.as_str())
        {
            return None;
        }
        Some(WorkspaceChangeResult {
            state,
            requires_confirmation: false,
            confirmation_token: String::new(),
            active_resources: ActiveResourceSummary::default(),
            warning: String::new(),
        })
    }
}

/// Applies a preferences patch and, when the patch turns the startup opt-in
/// on while a project is open, records that project as the one to reopen.
/// Without that the setting does nothing until the user happens to reopen the
/// same project once, so the next launch lands on Welcome instead. It takes
/// the store and the open root so the transition is provable against a
/// temporary store, while the command owns resolving the process-global home.
///
/// # Errors
/// Returns an error when the patch cannot be applied.
pub(crate) fn apply_preferences(
    store: &GlobalStore,
    patch: &Value,
    open_root: Option<&str>,
) -> AppResult<PreferencesDocumentV1> {
    let opted_in = preferences(store).preferences.startup.restore_last_project;
    let document = set_preferences(store, patch)?;
    if !opted_in
        && document.preferences.startup.restore_last_project
        && let Some(root) = open_root
        && let Some(recorded) = record_last_project_in(store, &json!(root))
    {
        return Ok(recorded);
    }
    Ok(document)
}

/// Records, or with a null root clears, the project startup may reopen.
/// Only the write is gated on the opt-in — a filesystem path nobody asked us
/// to keep is not ours to persist — while the clear is unconditional, so an
/// explicit close never leaves behind a root a later opt-in would silently
/// reopen. Best effort: `None` means nothing was written.
pub(crate) fn record_last_project_in(
    store: &GlobalStore,
    root: &Value,
) -> Option<PreferencesDocumentV1> {
    if !root.is_null() && !preferences(store).preferences.startup.restore_last_project {
        return None;
    }
    set_preferences(store, &json!({ "startup": { "lastProjectRoot": root } })).ok()
}

/// Deletes every app-scoped record and returns the manifest of what went. It
/// takes the store so the delete set is provable against a temporary one,
/// while the command owns resolving the process-global home.
pub(crate) fn reset_application_records(store: &GlobalStore) -> AppResult<[&'static str; 4]> {
    reset_preferences(store)?;
    reset_window_layout(store)?;
    store.delete_config(crate::update_preference_key())?;
    Ok([
        "preferences",
        "updates.auto-check",
        "window-state",
        "layout-state",
    ])
}

fn sanitize_recent_open_error(error: AppError) -> AppError {
    match error {
        AppError::Io(error) if error.kind() == std::io::ErrorKind::NotFound => {
            AppError::Message("recent-project-folder-not-found".to_owned())
        }
        AppError::Io(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            AppError::Message("recent-project-permission-required".to_owned())
        }
        AppError::Message(message) if message == "invalid or expired workspace confirmation" => {
            AppError::Message(message)
        }
        AppError::NoProject
        | AppError::NotImplemented(_)
        | AppError::Io(_)
        | AppError::ScratchpadConflict(_)
        | AppError::Message(_) => AppError::Message("recent-project-open-failed".to_owned()),
    }
}
