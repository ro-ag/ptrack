use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const ASSOCIATION_VERSION_V1: u8 = 1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AssociationError {
    HostRequired,
    UnsupportedVersion(u8),
    InvalidTarget(String),
    Stale(Option<&'static str>),
    ResolveRoot(String),
    CanonicalizeRoot(String),
}

impl fmt::Display for AssociationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HostRequired => formatter.write_str("association host is required"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported association version: {version}")
            }
            Self::InvalidTarget(detail) => {
                write!(formatter, "invalid association target: {detail}")
            }
            Self::Stale(Some(detail)) => write!(formatter, "stale association: {detail}"),
            Self::Stale(None) => formatter.write_str("stale association"),
            Self::ResolveRoot(detail) => {
                write!(formatter, "resolve association project root: {detail}")
            }
            Self::CanonicalizeRoot(detail) => {
                write!(formatter, "canonicalize association project root: {detail}")
            }
        }
    }
}

impl std::error::Error for AssociationError {}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssociationPointer {
    pub version: u8,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub plan_id: u64,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub task_id: u64,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssociationTarget {
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub plan_id: u64,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub task_id: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Association {
    pub version: u8,
    pub project_root: String,
    pub generation: u64,
    pub live_id: String,
    pub target: AssociationTarget,
    pub revision: u64,
}

pub trait AssociationCatalog: Send + Sync {
    /// Confirms that `plan_id` exists.
    ///
    /// # Errors
    ///
    /// Returns a content-free lookup error when the plan is unavailable.
    fn validate_plan(&self, plan_id: u64) -> Result<(), String>;
    /// Returns the authoritative parent plan for `task_id`.
    ///
    /// # Errors
    ///
    /// Returns a content-free lookup error when the task is unavailable.
    fn task_plan(&self, task_id: u64) -> Result<u64, String>;
}

pub struct AssociationHost<'a> {
    project_root: PathBuf,
    generation: u64,
    catalog: Option<&'a dyn AssociationCatalog>,
}

impl<'a> AssociationHost<'a> {
    /// Canonicalizes a project root and creates a generation-fenced host.
    ///
    /// # Errors
    ///
    /// Returns an error for zero generations or roots that cannot be resolved.
    pub fn new(
        project_root: impl AsRef<Path>,
        generation: u64,
        catalog: Option<&'a dyn AssociationCatalog>,
    ) -> Result<Self, AssociationError> {
        if generation == 0 {
            return Err(AssociationError::Stale(Some(
                "workspace generation must be nonzero",
            )));
        }
        let requested = project_root.as_ref();
        let absolute = if requested.is_absolute() {
            requested.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|error| AssociationError::ResolveRoot(error.to_string()))?
                .join(requested)
        };
        let canonical = std::fs::canonicalize(absolute)
            .map_err(|error| AssociationError::CanonicalizeRoot(error.to_string()))?;
        Ok(Self {
            project_root: canonical,
            generation,
            catalog,
        })
    }

    #[must_use]
    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Validates a pointer and mints descriptive association metadata.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid targets or stale previous metadata.
    pub fn bind(
        &self,
        live_id: &str,
        pointer: AssociationPointer,
        previous: Option<&Association>,
    ) -> Result<Association, AssociationError> {
        let live_id = live_id.trim();
        if live_id.is_empty() {
            return Err(AssociationError::InvalidTarget(
                "live identity is required".to_owned(),
            ));
        }
        let target = self.validate(pointer)?;
        let revision = if let Some(previous) = previous {
            if previous.version != ASSOCIATION_VERSION_V1
                || Path::new(&previous.project_root) != self.project_root
                || previous.generation != self.generation
                || previous.live_id != live_id
                || previous.revision == 0
                || previous.revision == u64::MAX
            {
                return Err(AssociationError::Stale(None));
            }
            previous.revision + 1
        } else {
            1
        };
        Ok(Association {
            version: ASSOCIATION_VERSION_V1,
            project_root: self.project_root.to_string_lossy().into_owned(),
            generation: self.generation,
            live_id: live_id.to_owned(),
            target,
            revision,
        })
    }

    /// Validates an authority-free pointer against the host catalog.
    ///
    /// # Errors
    ///
    /// Returns an error for unsupported versions or invalid plan/task links.
    pub fn validate(
        &self,
        pointer: AssociationPointer,
    ) -> Result<AssociationTarget, AssociationError> {
        if pointer.version != ASSOCIATION_VERSION_V1 {
            return Err(AssociationError::UnsupportedVersion(pointer.version));
        }
        if pointer.task_id != 0 && pointer.plan_id == 0 {
            return Err(AssociationError::InvalidTarget(
                "task requires a plan".to_owned(),
            ));
        }
        if pointer.plan_id == 0 {
            return Ok(AssociationTarget::default());
        }
        let catalog = self.catalog.ok_or_else(|| {
            AssociationError::InvalidTarget("project catalog is unavailable".to_owned())
        })?;
        catalog.validate_plan(pointer.plan_id).map_err(|error| {
            AssociationError::InvalidTarget(format!("plan #{}: {error}", pointer.plan_id))
        })?;
        if pointer.task_id != 0 {
            let actual = catalog.task_plan(pointer.task_id).map_err(|error| {
                AssociationError::InvalidTarget(format!("task #{}: {error}", pointer.task_id))
            })?;
            if actual != pointer.plan_id {
                return Err(AssociationError::InvalidTarget(format!(
                    "task #{} belongs to plan #{actual}, not plan #{}",
                    pointer.task_id, pointer.plan_id
                )));
            }
        }
        Ok(AssociationTarget {
            plan_id: pointer.plan_id,
            task_id: pointer.task_id,
        })
    }
}

const fn is_zero_u64(value: &u64) -> bool {
    *value == 0
}

/// Returns the canonical host root, or an empty path for no host.
#[must_use]
pub fn association_project_root<'host>(host: Option<&'host AssociationHost<'_>>) -> &'host Path {
    host.map_or_else(|| Path::new(""), AssociationHost::project_root)
}

/// Returns the workspace generation, or zero for no host.
#[must_use]
pub fn association_generation(host: Option<&AssociationHost<'_>>) -> u64 {
    host.map_or(0, AssociationHost::generation)
}

/// Binds through an optional host so absence fails closed like a nil Go host.
///
/// # Errors
///
/// Returns [`AssociationError::HostRequired`] when `host` is absent, and the
/// same validation/staleness errors as [`AssociationHost::bind`] otherwise.
pub fn bind_association(
    host: Option<&AssociationHost<'_>>,
    live_id: &str,
    pointer: AssociationPointer,
    previous: Option<&Association>,
) -> Result<Association, AssociationError> {
    host.ok_or(AssociationError::HostRequired)?
        .bind(live_id, pointer, previous)
}
