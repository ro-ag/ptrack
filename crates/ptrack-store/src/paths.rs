use std::path::{Component, Path, PathBuf};

use crate::{StoreError, StoreResult};

pub(crate) fn lexical_absolute(path: &Path, base: &Path) -> StoreResult<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    let mut clean = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::Prefix(prefix) => clean.push(prefix.as_os_str()),
            Component::RootDir => clean.push(component.as_os_str()),
            Component::CurDir => {}
            Component::Normal(value) => clean.push(value),
            Component::ParentDir => {
                if !clean.pop() {
                    return Err(StoreError::InvalidManifest(
                        "path escapes its filesystem root".to_owned(),
                    ));
                }
            }
        }
    }
    if clean.is_absolute() {
        Ok(clean)
    } else {
        Err(StoreError::InvalidManifest(
            "path could not be made absolute".to_owned(),
        ))
    }
}
