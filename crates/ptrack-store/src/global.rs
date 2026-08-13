use std::path::Path;
use std::sync::Arc;

use ptrack_core::{ProjectRef, Timestamp};

use crate::paths::lexical_absolute;
use crate::typed;
use crate::{
    ActivatedStore, ActiveBinding, Clock, Collection, LEGACY_CODEC_RAW, RecordEnvelope, RecordKey,
    StagedStore, Store, StoreError, StoreKind, StoreResult, SystemClock,
};

pub struct GlobalStore {
    active: ActivatedStore,
    clock: Arc<dyn Clock>,
}

impl GlobalStore {
    pub fn create_new(path: impl AsRef<Path>, binding: ActiveBinding) -> StoreResult<Self> {
        Self::create_new_with_clock(path, binding, SystemClock)
    }

    pub fn create_new_with_clock(
        path: impl AsRef<Path>,
        binding: ActiveBinding,
        clock: impl Clock + 'static,
    ) -> StoreResult<Self> {
        if binding.kind != StoreKind::Global {
            return Err(StoreError::ActivationBinding(
                "global store requires global binding".to_owned(),
            ));
        }
        let store = Store::create_new(path, StoreKind::Global)?;
        store.activate(&binding)?;
        Ok(Self {
            active: ActivatedStore { store, binding },
            clock: Arc::new(clock),
        })
    }

    pub fn activate(staged: StagedStore, binding: ActiveBinding) -> StoreResult<Self> {
        let active = staged.activate(binding)?;
        if active.binding.kind != StoreKind::Global {
            return Err(StoreError::ActivationBinding(
                "global store requires global binding".to_owned(),
            ));
        }
        Ok(Self {
            active,
            clock: Arc::new(SystemClock),
        })
    }

    pub fn open_existing(path: impl AsRef<Path>, binding: &ActiveBinding) -> StoreResult<Self> {
        let active = ActivatedStore::open(path, binding)?;
        if active.binding.kind != StoreKind::Global {
            return Err(StoreError::ActivationBinding(
                "global store requires global binding".to_owned(),
            ));
        }
        Ok(Self {
            active,
            clock: Arc::new(SystemClock),
        })
    }

    pub fn set_config(&self, key: &[u8], value: &[u8]) -> StoreResult<()> {
        if key.is_empty() {
            return Err(StoreError::InvalidManifest(
                "global config key must be nonempty".to_owned(),
            ));
        }
        self.active.write(|tx| {
            tx.put(
                Collection::GlobalConfig,
                RecordKey::Bytes(key),
                &RecordEnvelope::new(LEGACY_CODEC_RAW, 0, value),
            )?;
            Ok(())
        })
    }

    pub fn config(&self, key: &[u8]) -> StoreResult<Vec<u8>> {
        self.active.store.read(|tx| {
            Ok(tx
                .get(Collection::GlobalConfig, RecordKey::Bytes(key))?
                .map_or_else(Vec::new, |v| v.payload().to_vec()))
        })
    }

    pub fn register_project(
        &self,
        name: impl Into<String>,
        path: impl AsRef<Path>,
    ) -> StoreResult<ProjectRef> {
        let path = path.as_ref();
        let absolute = lexical_absolute(path, &std::env::current_dir()?)?;
        let path = absolute
            .to_str()
            .ok_or_else(|| StoreError::InvalidManifest("project path must be UTF-8".to_owned()))?
            .to_owned();
        let value = ProjectRef {
            name: name.into(),
            path: path.clone(),
            last_seen: self.clock.now_local(),
        };
        self.active.write(|tx| {
            typed::put(tx, RecordKey::Bytes(path.as_bytes()), &value)?;
            Ok(())
        })?;
        Ok(value)
    }

    pub fn projects(&self) -> StoreResult<Vec<ProjectRef>> {
        self.active.store.read(|tx| {
            let mut values = typed::scan::<ProjectRef>(tx)?;
            values.sort_by_key(|value| std::cmp::Reverse(timestamp_key(value.last_seen)));
            Ok(values)
        })
    }

    pub fn recent_projects(&self, limit: usize) -> StoreResult<Vec<ProjectRef>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut values = self.projects()?;
        values.truncate(limit.min(100));
        Ok(values)
    }

    pub fn record_backup(&self, key: i64, project: &Path, backup: &Path) -> StoreResult<()> {
        let project = project
            .to_str()
            .ok_or_else(|| StoreError::InvalidManifest("project path must be UTF-8".to_owned()))?;
        let backup = backup
            .to_str()
            .ok_or_else(|| StoreError::InvalidManifest("backup path must be UTF-8".to_owned()))?;
        let value = format!("{project}\t{backup}");
        let key = key.to_string();
        self.active.write(|tx| {
            tx.put(
                Collection::GlobalBackups,
                RecordKey::Bytes(key.as_bytes()),
                &RecordEnvelope::new(LEGACY_CODEC_RAW, 0, value.as_bytes()),
            )?;
            Ok(())
        })
    }
}

fn timestamp_key(value: Timestamp) -> i128 {
    value.unix_nanoseconds().unwrap_or(i128::MIN)
}
