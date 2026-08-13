use std::path::Path;

use crate::{ProjectStore, StoreResult};

impl ProjectStore {
    pub(crate) fn backup_to_with_after_copy(
        &self,
        destination: &Path,
        after_copy: impl FnOnce(&Path) -> StoreResult<()>,
    ) -> StoreResult<()> {
        self.backup_to_inner(destination, after_copy)
    }
}
