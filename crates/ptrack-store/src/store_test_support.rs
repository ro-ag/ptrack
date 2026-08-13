use std::path::Path;

use crate::{ImportData, ImportReport, JsonStageImportData, Store, StoreKind, StoreResult};

impl Store {
    #[cfg_attr(not(unix), allow(dead_code))]
    pub(crate) fn create_new_with_creation_hooks(
        path: impl AsRef<Path>,
        kind: StoreKind,
        before_create: impl FnOnce() -> StoreResult<()>,
        after_create: impl FnOnce() -> StoreResult<()>,
    ) -> StoreResult<Self> {
        Self::create_new_inner(path.as_ref(), kind, before_create, after_create)
    }

    pub(crate) fn import_json_stage_new_with_before_ready(
        path: impl AsRef<Path>,
        data: JsonStageImportData,
        before_ready: impl FnOnce() -> StoreResult<()>,
    ) -> StoreResult<(Self, ImportReport)> {
        Self::import_json_stage_new_inner(path.as_ref(), data, before_ready)
    }

    pub(crate) fn import_new_with_before_ready(
        path: impl AsRef<Path>,
        data: ImportData,
        before_ready: impl FnOnce() -> StoreResult<()>,
    ) -> StoreResult<(Self, ImportReport)> {
        Self::import_new_inner(
            path.as_ref(),
            data,
            || Ok(()),
            || Ok(()),
            before_ready,
            || Ok(()),
        )
    }

    #[cfg_attr(not(unix), allow(dead_code))]
    pub(crate) fn import_new_with_parent_hooks(
        path: impl AsRef<Path>,
        data: ImportData,
        before_create: impl FnOnce() -> StoreResult<()>,
        after_create: impl FnOnce() -> StoreResult<()>,
        before_ready: impl FnOnce() -> StoreResult<()>,
        after_ready: impl FnOnce() -> StoreResult<()>,
    ) -> StoreResult<(Self, ImportReport)> {
        Self::import_new_inner(
            path.as_ref(),
            data,
            before_create,
            after_create,
            before_ready,
            after_ready,
        )
    }
}
