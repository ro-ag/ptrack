use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{ImportError, ImportResult, invalid};

pub(crate) const FORMAT: &str = "ptrack-db-stage";
pub(crate) const VERSION: &str = "1";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    pub format: String,
    pub version: String,
    pub database_count: String,
    pub quarantine_count: String,
    pub registry: Vec<RegistryEntry>,
    pub databases: Vec<DatabaseEntry>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RegistryEntry {
    pub source_path: String,
    pub canonical_root: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DatabaseEntry {
    pub id: String,
    pub kind: DatabaseKind,
    pub project_root: Option<String>,
    pub source_path: String,
    pub source_format: String,
    pub source_identity: SourceIdentity,
    pub data: Artifact,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum DatabaseKind {
    Global,
    Project,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SourceIdentity {
    pub device: String,
    pub inode: String,
    pub size: String,
    pub mtime_seconds: String,
    pub mtime_nanos: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Artifact {
    pub path: String,
    pub sha256: String,
    pub bytes: String,
    pub record_count: String,
    pub bucket_count: String,
}

pub(crate) fn decode_manifest(bytes: &[u8]) -> ImportResult<Manifest> {
    if !bytes.ends_with(b"\n") || bytes[..bytes.len().saturating_sub(1)].contains(&b'\n') {
        return invalid("manifest must be one compact JSON object followed by one newline");
    }
    let manifest: Manifest = serde_json::from_slice(bytes)
        .map_err(|error| ImportError::InvalidStage(format!("decode manifest.json: {error}")))?;
    manifest.validate()?;
    Ok(manifest)
}

impl Manifest {
    fn validate(&self) -> ImportResult<()> {
        if self.format != FORMAT || self.version != VERSION {
            return invalid("unsupported manifest format or version");
        }
        let database_count = decimal_u64(&self.database_count, "database_count")?;
        let _ = decimal_u64(&self.quarantine_count, "quarantine_count")?;
        if database_count != self.databases.len() as u64 {
            return invalid("manifest database_count does not match databases");
        }
        if database_count == 0 || database_count > 10_000 {
            return invalid("manifest database_count is outside 1..=10000");
        }
        let mut previous_source = None;
        for entry in &self.registry {
            if !clean_absolute(Path::new(&entry.source_path))
                || !clean_absolute(Path::new(&entry.canonical_root))
            {
                return invalid("registry paths must be absolute and clean");
            }
            if previous_source.is_some_and(|prior| prior >= entry.source_path.as_str()) {
                return invalid("registry entries are not in strict source-path order");
            }
            previous_source = Some(entry.source_path.as_str());
        }
        let mut previous: Option<(&DatabaseKind, Option<&str>)> = None;
        let mut ids = BTreeSet::new();
        let mut artifacts = BTreeSet::new();
        for database in &self.databases {
            database.validate()?;
            let key = (&database.kind, database.project_root.as_deref());
            if previous.is_some_and(|prior| prior >= key) {
                return invalid(
                    "manifest databases are not in canonical global/project-root order",
                );
            }
            previous = Some(key);
            if !ids.insert(database.id.as_str()) || !artifacts.insert(database.data.path.as_str()) {
                return invalid("manifest database ids and data paths must be unique");
            }
        }
        let registry_roots = self
            .registry
            .iter()
            .map(|entry| entry.canonical_root.as_str())
            .collect::<BTreeSet<_>>();
        let project_roots = self
            .databases
            .iter()
            .filter_map(|database| database.project_root.as_deref())
            .collect::<BTreeSet<_>>();
        if registry_roots != project_roots {
            return invalid("manifest projects do not match canonical registry roots");
        }
        Ok(())
    }
}

impl DatabaseEntry {
    fn validate(&self) -> ImportResult<()> {
        validate_id(&self.id)?;
        match (self.kind, self.project_root.as_deref()) {
            (DatabaseKind::Global, None) => {}
            (DatabaseKind::Global, Some(_)) => {
                return invalid("global database project_root must be null");
            }
            (DatabaseKind::Project, Some(root)) if clean_absolute(Path::new(root)) => {}
            (DatabaseKind::Project, _) => {
                return invalid("project database requires an absolute clean project_root");
            }
        }
        if !clean_absolute(Path::new(&self.source_path)) {
            return invalid("database source_path must be absolute and clean");
        }
        decimal_u64(&self.source_format, "source_format")?;
        self.source_identity.validate()?;
        self.data.validate()?;
        if !self.data.path.starts_with("databases/") {
            return invalid("data artifact is outside the databases staging directory");
        }
        Ok(())
    }
}

impl SourceIdentity {
    fn validate(&self) -> ImportResult<()> {
        for (value, field) in [
            (&self.device, "source_identity.device"),
            (&self.inode, "source_identity.inode"),
            (&self.size, "source_identity.size"),
            (&self.mtime_nanos, "source_identity.mtime_nanos"),
        ] {
            decimal_u64(value, field)?;
        }
        decimal_i64(&self.mtime_seconds, "source_identity.mtime_seconds")?;
        if decimal_u64(&self.mtime_nanos, "source_identity.mtime_nanos")? >= 1_000_000_000 {
            return invalid("source_identity.mtime_nanos must be below one billion");
        }
        digest(&self.sha256, "source_identity.sha256")
    }
}

impl Artifact {
    fn validate(&self) -> ImportResult<()> {
        relative_artifact_path(&self.path)?;
        digest(&self.sha256, "artifact.sha256")?;
        decimal_u64(&self.bytes, "artifact.bytes")?;
        decimal_u64(&self.record_count, "artifact.record_count")?;
        decimal_u64(&self.bucket_count, "artifact.bucket_count")?;
        Ok(())
    }

    pub(crate) fn resolved(&self, root: &Path) -> PathBuf {
        root.join(&self.path)
    }
}

pub(crate) fn decimal_u64(value: &str, field: &str) -> ImportResult<u64> {
    if value.is_empty()
        || (value.len() > 1 && value.starts_with('0'))
        || !value.bytes().all(|b| b.is_ascii_digit())
    {
        return invalid(format!("{field} is not canonical unsigned decimal"));
    }
    value
        .parse()
        .map_err(|_| ImportError::InvalidStage(format!("{field} is outside u64")))
}

pub(crate) fn decimal_i64(value: &str, field: &str) -> ImportResult<i64> {
    let parsed = decimal_i128(value, field)?;
    i64::try_from(parsed).map_err(|_| ImportError::InvalidStage(format!("{field} is outside i64")))
}

fn decimal_i128(value: &str, field: &str) -> ImportResult<i128> {
    let digits = value.strip_prefix('-').unwrap_or(value);
    if digits.is_empty()
        || (digits.len() > 1 && digits.starts_with('0'))
        || value == "-0"
        || !digits.bytes().all(|b| b.is_ascii_digit())
    {
        return invalid(format!("{field} is not canonical signed decimal"));
    }
    value
        .parse()
        .map_err(|_| ImportError::InvalidStage(format!("{field} is outside i128")))
}

pub(crate) fn digest(value: &str, field: &str) -> ImportResult<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return invalid(format!("{field} is not lowercase SHA-256 hex"));
    }
    Ok(())
}

fn validate_id(value: &str) -> ImportResult<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return invalid("database id is not canonical");
    }
    Ok(())
}

fn relative_artifact_path(value: &str) -> ImportResult<()> {
    let path = Path::new(value);
    let components = path.components().collect::<Vec<_>>();
    if value.contains('\\')
        || path.is_absolute()
        || components.len() != 2
        || components.first().and_then(|component| match component {
            Component::Normal(value) => value.to_str(),
            _ => None,
        }) != Some("databases")
        || components
            .iter()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return invalid("artifact path must be a clean relative slash path");
    }
    Ok(())
}

pub(crate) fn clean_absolute(path: &Path) -> bool {
    path.is_absolute()
        && path.components().collect::<PathBuf>() == path
        && path.components().all(|component| {
            matches!(
                component,
                Component::Prefix(_) | Component::RootDir | Component::Normal(_)
            )
        })
}
