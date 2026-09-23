//! The pinned guide publisher: reads and atomically replaces project guide
//! files through directory handles pinned to the identity the preview saw.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use ptrack_store::{PinnedProjectDirectory, PrivatePathIdentity};

use super::guide::{DesktopGuideFileManifest, GuideFileSnapshot, require_guide_base};
use super::{GUIDE_FILE_LIMIT, GUIDE_PREVIEW_STALE, content_digest, random_id, recovery};
use crate::{AppError, AppResult};

pub(super) struct PinnedGuideRoot<'a> {
    path: Option<PathBuf>,
    pinned: Option<&'a PinnedProjectDirectory>,
    identity: PrivatePathIdentity,
    handle: fs::File,
    staging: Option<fs::File>,
}

#[cfg(unix)]
impl<'a> PinnedGuideRoot<'a> {
    pub(super) fn capture(path: &Path, expected: PrivatePathIdentity) -> AppResult<Self> {
        use std::os::unix::fs::MetadataExt as _;

        let metadata = fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink()
            || !metadata.is_dir()
            || fs::canonicalize(path)? != path
        {
            return Err(AppError::Message(GUIDE_PREVIEW_STALE.to_owned()));
        }
        let handle = fs::File::open(path)?;
        let identity = PrivatePathIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        };
        let opened = handle.metadata()?;
        if identity != expected || opened.dev() != identity.device || opened.ino() != identity.inode
        {
            return Err(AppError::Message(GUIDE_PREVIEW_STALE.to_owned()));
        }
        Ok(Self {
            path: Some(path.to_owned()),
            pinned: None,
            identity,
            handle,
            staging: None,
        })
    }

    pub(super) fn from_pinned(
        pinned: &'a PinnedProjectDirectory,
        expected: PrivatePathIdentity,
    ) -> AppResult<Self> {
        use std::os::unix::fs::MetadataExt as _;

        pinned.verify().map_err(recovery)?;
        if pinned.root_identity() != expected {
            return Err(AppError::Message(GUIDE_PREVIEW_STALE.to_owned()));
        }
        let handle = pinned.try_clone_root_directory().map_err(recovery)?;
        let staging = pinned.try_clone_project_directory().map_err(recovery)?;
        let metadata = handle.metadata()?;
        if metadata.dev() != expected.device || metadata.ino() != expected.inode {
            return Err(AppError::Message(GUIDE_PREVIEW_STALE.to_owned()));
        }
        Ok(Self {
            path: None,
            pinned: Some(pinned),
            identity: expected,
            handle,
            staging: Some(staging),
        })
    }

    pub(super) fn verify(&self) -> AppResult<()> {
        use std::os::unix::fs::MetadataExt as _;

        let opened = self.handle.metadata()?;
        if opened.dev() != self.identity.device || opened.ino() != self.identity.inode {
            return Err(AppError::Message(GUIDE_PREVIEW_STALE.to_owned()));
        }
        if let Some(pinned) = self.pinned {
            pinned.verify().map_err(recovery)?;
        } else if let Some(path) = &self.path {
            let metadata = fs::symlink_metadata(path)?;
            if metadata.file_type().is_symlink()
                || !metadata.is_dir()
                || metadata.dev() != self.identity.device
                || metadata.ino() != self.identity.inode
            {
                return Err(AppError::Message(GUIDE_PREVIEW_STALE.to_owned()));
            }
        }
        Ok(())
    }

    pub(super) fn read(&self, name: &str) -> AppResult<Option<GuideFileSnapshot>> {
        use rustix::fs::{AtFlags, Mode, OFlags, openat, statat};
        use std::os::unix::fs::MetadataExt as _;

        let before = match statat(&self.handle, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => stat,
            Err(error) if error == rustix::io::Errno::NOENT => return Ok(None),
            Err(_) => return Err(AppError::Message(GUIDE_PREVIEW_STALE.to_owned())),
        };
        let descriptor = openat(
            &self.handle,
            name,
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(|_| AppError::Message(GUIDE_PREVIEW_STALE.to_owned()))?;
        let file = fs::File::from(descriptor);
        let metadata = file.metadata()?;
        let identity = PrivatePathIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        };
        if !metadata.is_file() || !guide_stat_matches(identity, &before) {
            return Err(AppError::Message(GUIDE_PREVIEW_STALE.to_owned()));
        }
        let mode = metadata.mode() & 0o7777;
        let mut bytes = Vec::with_capacity(
            usize::try_from(metadata.len().min(GUIDE_FILE_LIMIT)).unwrap_or_default(),
        );
        file.take(GUIDE_FILE_LIMIT + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > GUIDE_FILE_LIMIT {
            return Err(AppError::Message(
                "project guide file exceeds its byte limit".to_owned(),
            ));
        }
        let after = statat(&self.handle, name, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|_| AppError::Message(GUIDE_PREVIEW_STALE.to_owned()))?;
        if !guide_stat_matches(identity, &after) {
            return Err(AppError::Message(GUIDE_PREVIEW_STALE.to_owned()));
        }
        let content = String::from_utf8(bytes)
            .map_err(|_| AppError::Message("project guide file is not valid UTF-8".to_owned()))?;
        Ok(Some(GuideFileSnapshot {
            identity,
            digest: content_digest(content.as_bytes()),
            content,
            mode,
        }))
    }

    pub(super) fn publish(
        &self,
        manifest: &DesktopGuideFileManifest,
        content: &str,
    ) -> AppResult<()> {
        use rustix::fs::{
            AtFlags, Mode, OFlags, RenameFlags, openat, renameat, renameat_with, unlinkat,
        };
        use std::os::unix::fs::PermissionsExt as _;

        let staging = self
            .staging
            .as_ref()
            .ok_or_else(|| recovery("project guide staging authority is unavailable"))?;
        let temporary = format!(".guide-{}-{}.tmp", manifest.name, random_id()?);
        let descriptor = openat(
            staging,
            temporary.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::from_raw_mode(platform_raw_mode(manifest.mode)),
        )
        .map_err(|error| AppError::Io(error.into()))?;
        let mut file = fs::File::from(descriptor);
        let prepared = (|| -> AppResult<()> {
            file.write_all(content.as_bytes())?;
            file.set_permissions(fs::Permissions::from_mode(manifest.mode))?;
            file.sync_all()?;
            Ok(())
        })();
        drop(file);
        if let Err(error) = prepared {
            let _ = unlinkat(staging, temporary.as_str(), AtFlags::empty());
            return Err(error);
        }
        #[cfg(test)]
        super::test_support::run_guide_before_publish_hook();
        let mut staged = true;
        let publication = (|| -> AppResult<()> {
            let current = self.read(&manifest.name)?;
            require_guide_base(current.as_ref(), manifest)?;
            self.verify()?;
            let published = if manifest.base_identity.is_none() {
                renameat_with(
                    staging,
                    temporary.as_str(),
                    &self.handle,
                    manifest.name.as_str(),
                    RenameFlags::NOREPLACE,
                )
            } else {
                renameat(
                    staging,
                    temporary.as_str(),
                    &self.handle,
                    manifest.name.as_str(),
                )
            };
            if published.is_err() {
                return Err(AppError::Message(GUIDE_PREVIEW_STALE.to_owned()));
            }
            staged = false;
            Ok(())
        })();
        if let Err(error) = publication {
            if staged {
                let _ = unlinkat(staging, temporary.as_str(), AtFlags::empty());
            }
            return Err(error);
        }
        if staged {
            let _ = unlinkat(staging, temporary.as_str(), AtFlags::empty());
            return Err(AppError::Message(
                "project guide publication failed".to_owned(),
            ));
        }
        self.handle.sync_all()?;
        staging.sync_all()?;
        let applied = self.read(&manifest.name)?;
        if applied
            .as_ref()
            .is_none_or(|snapshot| snapshot.digest != manifest.output_digest)
        {
            return Err(AppError::Message(GUIDE_PREVIEW_STALE.to_owned()));
        }
        Ok(())
    }
}

#[cfg(unix)]
fn guide_stat_matches(identity: PrivatePathIdentity, stat: &rustix::fs::Stat) -> bool {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let device_matches = stat.st_dev == identity.device;
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    let device_matches = u64::try_from(stat.st_dev).is_ok_and(|device| device == identity.device);
    device_matches && stat.st_ino == identity.inode
}

#[cfg(any(target_os = "linux", target_os = "android"))]
const fn platform_raw_mode(mode: u32) -> u32 {
    mode
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
fn platform_raw_mode(mode: u32) -> u16 {
    u16::try_from(mode).expect("mode bits fit the platform raw mode")
}
