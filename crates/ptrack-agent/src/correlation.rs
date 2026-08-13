use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{ASSOCIATION_VERSION_V1, Run};

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventCorrelation {
    pub project_root: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub repository_root: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub terminal_id: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub plan_id: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub task_id: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub generation: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub association_revision: u64,
}

#[must_use]
pub fn discover_repository_root(project_root: impl AsRef<Path>) -> Option<PathBuf> {
    let mut current = std::fs::canonicalize(project_root).ok()?;
    loop {
        let marker = current.join(".git");
        if let Ok(metadata) = std::fs::metadata(marker)
            && (metadata.is_dir() || metadata.is_file())
        {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

#[must_use]
pub fn event_correlation_for_run(run: &Run, repository_root: Option<&Path>) -> EventCorrelation {
    let mut result = EventCorrelation {
        project_root: run.project_root.clone(),
        repository_root: repository_root
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default(),
        terminal_id: run.terminal_id.clone(),
        ..EventCorrelation::default()
    };
    let Some(association) = run.association.as_ref() else {
        return result;
    };
    if association.version != ASSOCIATION_VERSION_V1
        || association.project_root != run.project_root
        || association.live_id != run.id
        || association.generation == 0
        || association.revision == 0
        || (association.target.task_id != 0 && association.target.plan_id == 0)
    {
        return result;
    }
    result.plan_id = association.target.plan_id;
    result.task_id = association.target.task_id;
    result.generation = association.generation;
    result.association_revision = association.revision;
    result
}

const fn is_zero(value: &u64) -> bool {
    *value == 0
}
