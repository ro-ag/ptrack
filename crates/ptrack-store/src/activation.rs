use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use crate::schema::{
    MANIFEST_KEY_ACTIVATION_GENERATION, MANIFEST_KEY_APPLICATION_WRITES,
    MANIFEST_KEY_CANONICAL_PATH, MANIFEST_KEY_DATABASE_ID, MANIFEST_KEY_STATE, STORE_STATE_ACTIVE,
};
use crate::{JsonStageProvenance, Store, StoreError, StoreKind, StoreResult, WriteTransaction};

/// The exact runtime identity permitted to open a database for application writes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveBinding {
    pub generation: u64,
    pub database_id: String,
    pub kind: StoreKind,
    pub canonical_path: PathBuf,
}

/// An immutable imported candidate. It has no application-write API.
pub struct StagedStore {
    store: Store,
}

/// A store activated for one exact runtime binding.
pub struct ActivatedStore {
    inner: Arc<ActivatedStoreInner>,
}

struct ActivatedStoreInner {
    pub(crate) store: Store,
    pub(crate) binding: ActiveBinding,
}

impl StagedStore {
    /// Opens only an inactive JSON-stage candidate.
    pub fn open(path: impl AsRef<Path>, kind: StoreKind) -> StoreResult<Self> {
        let store = Store::open_existing(path, kind)?;
        if store.json_stage_provenance()?.is_none() {
            return Err(StoreError::ActivationBinding(
                "store is not a JSON-stage candidate".to_owned(),
            ));
        }
        if store.active_binding()?.is_some() {
            return Err(StoreError::ActivationBinding(
                "candidate is already active".to_owned(),
            ));
        }
        Ok(Self { store })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        self.store.path()
    }

    #[must_use]
    pub fn kind(&self) -> StoreKind {
        self.store.kind()
    }

    pub fn provenance(&self) -> StoreResult<JsonStageProvenance> {
        self.store.json_stage_provenance()?.ok_or_else(|| {
            StoreError::ActivationBinding("store lost JSON-stage provenance".to_owned())
        })
    }

    /// Permanently consumes the staged state and binds this disposable candidate.
    pub fn activate(self, binding: ActiveBinding) -> StoreResult<ActivatedStore> {
        self.store.activate(&binding)?;
        ActivatedStore::new(self.store, binding)
    }
}

impl ActivatedStore {
    pub(crate) fn new(store: Store, binding: ActiveBinding) -> StoreResult<Self> {
        let inner = Arc::new(ActivatedStoreInner { store, binding });
        let key = inner.binding.canonical_path.clone();
        let mut registry = activated_registry().lock().map_err(|_| {
            StoreError::ActivationBinding("store registry is unavailable".to_owned())
        })?;
        registry.retain(|_, value| value.strong_count() > 0);
        registry.insert(key, Arc::downgrade(&inner));
        Ok(Self { inner })
    }

    pub(crate) fn open(path: impl AsRef<Path>, expected: &ActiveBinding) -> StoreResult<Self> {
        validate_binding_for_path(expected, expected.kind, path.as_ref())?;
        let inner = activated_registry()
            .lock()
            .map_err(|_| StoreError::ActivationBinding("store registry is unavailable".to_owned()))?
            .get(&expected.canonical_path)
            .and_then(Weak::upgrade);
        if let Some(inner) = inner {
            inner.store.ensure_current_path()?;
            if inner.binding != *expected
                || inner.store.active_binding()?.as_ref() != Some(expected)
            {
                return Err(StoreError::ActivationBinding(
                    "stored binding does not match the active runtime".to_owned(),
                ));
            }
            return Ok(Self { inner });
        }
        let store = Store::open_existing(path, expected.kind)?;
        let actual = store
            .active_binding()?
            .ok_or_else(|| StoreError::ActivationBinding("store is not active".to_owned()))?;
        validate_binding_for_path(expected, expected.kind, store.path())?;
        if actual != *expected {
            return Err(StoreError::ActivationBinding(
                "stored binding does not match the active runtime".to_owned(),
            ));
        }
        Self::new(store, actual)
    }

    #[must_use]
    pub fn binding(&self) -> &ActiveBinding {
        &self.inner.binding
    }

    pub(crate) fn store(&self) -> &Store {
        &self.inner.store
    }

    pub fn application_writes(&self) -> StoreResult<bool> {
        self.inner.store.application_writes()
    }

    pub(crate) fn write<R>(
        &self,
        operation: impl FnOnce(&mut WriteTransaction) -> StoreResult<R>,
    ) -> StoreResult<R> {
        self.inner
            .store
            .write_application(&self.inner.binding, operation)
    }

    /// Performs activation-tool normalization without tripping the
    /// application-write rollback fuse. This is deliberately crate-private:
    /// runtime services must use [`Self::write`].
    pub(crate) fn activation_write<R>(
        &self,
        operation: impl FnOnce(&mut WriteTransaction) -> StoreResult<R>,
    ) -> StoreResult<R> {
        self.inner
            .store
            .write_activation(&self.inner.binding, operation)
    }
}

fn activated_registry() -> &'static Mutex<BTreeMap<PathBuf, Weak<ActivatedStoreInner>>> {
    static REGISTRY: OnceLock<Mutex<BTreeMap<PathBuf, Weak<ActivatedStoreInner>>>> =
        OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(BTreeMap::new()))
}

pub(crate) fn validate_binding_for_path(
    binding: &ActiveBinding,
    kind: StoreKind,
    path: &Path,
) -> StoreResult<()> {
    if binding.generation == 0 {
        return Err(StoreError::ActivationBinding(
            "generation must be nonzero".to_owned(),
        ));
    }
    if binding.kind != kind {
        return Err(StoreError::ActivationBinding(
            "store kind does not match".to_owned(),
        ));
    }
    if binding.database_id.is_empty()
        || binding.database_id.len() > 128
        || !binding
            .database_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return Err(StoreError::ActivationBinding(
            "database ID is not canonical".to_owned(),
        ));
    }
    if !is_clean_absolute(&binding.canonical_path) {
        return Err(StoreError::ActivationBinding(
            "canonical path must be absolute and lexically clean".to_owned(),
        ));
    }
    if binding.canonical_path.to_str().is_none() {
        return Err(StoreError::ActivationBinding(
            "canonical path must be valid UTF-8".to_owned(),
        ));
    }
    let actual = std::fs::canonicalize(path)?;
    if actual != binding.canonical_path {
        return Err(StoreError::ActivationBinding(
            "canonical path does not identify the opened database".to_owned(),
        ));
    }
    Ok(())
}

pub(crate) fn binding_from_manifest(
    entries: &BTreeMap<Vec<u8>, Vec<u8>>,
) -> StoreResult<Option<ActiveBinding>> {
    if entries.get(MANIFEST_KEY_STATE).map(Vec::as_slice) != Some(STORE_STATE_ACTIVE) {
        return Ok(None);
    }
    let generation = u64::from_be_bytes(
        entries
            .get(MANIFEST_KEY_ACTIVATION_GENERATION)
            .ok_or_else(|| {
                StoreError::InvalidManifest("activation generation is missing".to_owned())
            })?
            .as_slice()
            .try_into()
            .map_err(|_| {
                StoreError::InvalidManifest(
                    "activation generation must contain exactly eight bytes".to_owned(),
                )
            })?,
    );
    if generation == 0 {
        return Err(StoreError::InvalidManifest(
            "activation generation must be nonzero".to_owned(),
        ));
    }
    let database_id = std::str::from_utf8(
        entries
            .get(MANIFEST_KEY_DATABASE_ID)
            .ok_or_else(|| StoreError::InvalidManifest("database ID is missing".to_owned()))?,
    )
    .map_err(|_| StoreError::InvalidManifest("database ID is not UTF-8".to_owned()))?
    .to_owned();
    let kind = StoreKind::from_bytes(
        entries
            .get(crate::schema::MANIFEST_KEY_STORE_KIND)
            .ok_or_else(|| StoreError::InvalidManifest("store kind is missing".to_owned()))?,
    )?;
    let canonical_path =
        PathBuf::from(
            std::str::from_utf8(entries.get(MANIFEST_KEY_CANONICAL_PATH).ok_or_else(|| {
                StoreError::InvalidManifest("canonical path is missing".to_owned())
            })?)
            .map_err(|_| StoreError::InvalidManifest("canonical path is not UTF-8".to_owned()))?,
        );
    match entries
        .get(MANIFEST_KEY_APPLICATION_WRITES)
        .map(Vec::as_slice)
    {
        Some(b"true" | b"false") => {}
        _ => {
            return Err(StoreError::InvalidManifest(
                "application_writes must be true or false".to_owned(),
            ));
        }
    }
    if database_id.is_empty()
        || database_id.len() > 128
        || !database_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
        || !is_clean_absolute(&canonical_path)
        || canonical_path.to_str().is_none()
    {
        return Err(StoreError::InvalidManifest(
            "activation binding is not canonical".to_owned(),
        ));
    }
    Ok(Some(ActiveBinding {
        generation,
        database_id,
        kind,
        canonical_path,
    }))
}

fn is_clean_absolute(path: &Path) -> bool {
    path.is_absolute()
        && path.components().all(|component| {
            matches!(
                component,
                Component::Prefix(_) | Component::RootDir | Component::Normal(_)
            )
        })
}
