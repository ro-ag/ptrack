use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, Read};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

use rustix::fs::{AtFlags, FlockOperation, Mode, OFlags};

pub(super) struct PinnedRuntimeDir {
    display_path: PathBuf,
    directory: File,
    device: u64,
    inode: u64,
}

pub(super) struct DescriptorLock(File);

pub(super) struct OwnedFileIdentity {
    device: u64,
    inode: u64,
}

impl Drop for DescriptorLock {
    fn drop(&mut self) {
        let _ = rustix::fs::flock(&self.0, FlockOperation::Unlock);
    }
}

impl PinnedRuntimeDir {
    pub(super) fn open(path: &Path) -> io::Result<Self> {
        let descriptor = rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )?;
        let directory = File::from(descriptor);
        let metadata = directory.metadata()?;
        let pinned = Self {
            display_path: path.to_path_buf(),
            device: metadata.dev(),
            inode: metadata.ino(),
            directory,
        };
        pinned.verify()?;
        Ok(pinned)
    }

    pub(super) fn lock_private_descriptor(&self) -> io::Result<DescriptorLock> {
        self.verify()?;
        let descriptor = rustix::fs::openat(
            &self.directory,
            ".agent-registry.lock",
            OFlags::RDWR | OFlags::CREATE | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::RUSR | Mode::WUSR,
        )?;
        rustix::fs::fchmod(&descriptor, Mode::RUSR | Mode::WUSR)?;
        let file = File::from(descriptor);
        rustix::fs::flock(&file, FlockOperation::LockExclusive)?;
        if let Err(error) = self.verify() {
            let _ = rustix::fs::flock(&file, FlockOperation::Unlock);
            return Err(error);
        }
        Ok(DescriptorLock(file))
    }

    pub(super) fn create_private_file(&self, name: &str) -> io::Result<File> {
        self.verify()?;
        validate_name(OsStr::new(name))?;
        let descriptor = rustix::fs::openat(
            &self.directory,
            name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::RUSR | Mode::WUSR,
        )?;
        if let Err(error) = rustix::fs::fchmod(&descriptor, Mode::RUSR | Mode::WUSR) {
            drop(descriptor);
            let _ = rustix::fs::unlinkat(&self.directory, name, AtFlags::empty());
            return Err(error.into());
        }
        Ok(File::from(descriptor))
    }

    pub(super) fn read_private_file(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.verify()?;
        let name = self.path_name(path)?;
        let descriptor = rustix::fs::openat(
            &self.directory,
            name,
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )?;
        let mut contents = Vec::new();
        File::from(descriptor).read_to_end(&mut contents)?;
        Ok(contents)
    }

    pub(super) fn replace_private_descriptor(
        &self,
        temporary_name: &str,
        path: &Path,
    ) -> io::Result<()> {
        self.verify()?;
        validate_name(OsStr::new(temporary_name))?;
        let final_name = self.path_name(path)?;
        rustix::fs::renameat(&self.directory, temporary_name, &self.directory, final_name)?;
        Ok(())
    }

    pub(super) fn secure_published_descriptor(&self, path: &Path) -> io::Result<()> {
        self.verify()?;
        let name = self.path_name(path)?;
        let descriptor = rustix::fs::openat(
            &self.directory,
            name,
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )?;
        rustix::fs::fchmod(&descriptor, Mode::RUSR | Mode::WUSR)?;
        let stat = rustix::fs::fstat(&descriptor)?;
        if stat.st_mode & 0o077 != 0 {
            return Err(io::Error::other("AgentRun descriptor is not private"));
        }
        Ok(())
    }

    pub(super) fn remove_file(&self, name: &str) -> io::Result<()> {
        self.verify()?;
        validate_name(OsStr::new(name))?;
        rustix::fs::unlinkat(&self.directory, name, AtFlags::empty())?;
        Ok(())
    }

    pub(super) fn remove_path(&self, path: &Path) -> io::Result<()> {
        let name = self.path_name(path)?;
        self.remove_file(&name.to_string_lossy())
    }

    pub(super) fn remove_owned_file(
        &self,
        name: &str,
        identity: &OwnedFileIdentity,
    ) -> io::Result<()> {
        validate_name(OsStr::new(name))?;
        let descriptor = rustix::fs::openat(
            &self.directory,
            name,
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )?;
        let metadata = File::from(descriptor).metadata()?;
        if !metadata.is_file()
            || metadata.dev() != identity.device
            || metadata.ino() != identity.inode
        {
            return Err(io::Error::other(
                "AgentRun descriptor identity changed before cleanup",
            ));
        }
        rustix::fs::unlinkat(&self.directory, name, AtFlags::empty())?;
        self.directory.sync_all()
    }

    pub(super) fn remove_owned_path(
        &self,
        path: &Path,
        identity: &OwnedFileIdentity,
    ) -> io::Result<()> {
        let name = self.path_name(path)?;
        let name = name
            .to_str()
            .ok_or_else(|| io::Error::other("AgentRun descriptor name is not UTF-8"))?;
        self.remove_owned_file(name, identity)
    }

    pub(super) fn sync(&self) -> io::Result<()> {
        self.verify()?;
        self.directory.sync_all()
    }

    fn path_name<'path>(&self, path: &'path Path) -> io::Result<&'path OsStr> {
        if path.parent() != Some(self.display_path.as_path()) {
            return Err(io::Error::other(
                "AgentRun descriptor escaped its pinned runtime directory",
            ));
        }
        let name = path
            .file_name()
            .ok_or_else(|| io::Error::other("AgentRun descriptor has no filename"))?;
        validate_name(name)?;
        Ok(name)
    }

    fn verify(&self) -> io::Result<()> {
        let metadata = fs::metadata(&self.display_path)?;
        if !metadata.is_dir() || metadata.dev() != self.device || metadata.ino() != self.inode {
            return Err(io::Error::other(
                "AgentRun runtime directory changed while pinned",
            ));
        }
        Ok(())
    }
}

pub(super) fn file_identity(file: &File) -> io::Result<OwnedFileIdentity> {
    let metadata = file.metadata()?;
    Ok(OwnedFileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

pub(super) fn prepare_private_runtime_dir(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(io::Error::other(
            "AgentRun runtime directory is not a private directory",
        ));
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    if fs::symlink_metadata(path)?.permissions().mode() & 0o077 != 0 {
        return Err(io::Error::other(
            "AgentRun runtime directory is not private",
        ));
    }
    Ok(())
}

pub(super) fn read_private_file(path: &Path) -> io::Result<Vec<u8>> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("AgentRun descriptor has no parent"))?;
    PinnedRuntimeDir::open(parent)?.read_private_file(path)
}

fn validate_name(name: &OsStr) -> io::Result<()> {
    let mut components = Path::new(name).components();
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        return Err(io::Error::other("AgentRun descriptor name is not relative"));
    }
    Ok(())
}
