use std::ffi::c_void;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::path::PathBuf;
use std::ptr;

use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_ALL, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    EXPLICIT_ACCESS_W, SE_FILE_OBJECT, SET_ACCESS, SetEntriesInAclW, SetNamedSecurityInfoW,
    TRUSTEE_IS_SID, TRUSTEE_IS_USER, TRUSTEE_W,
};
use windows_sys::Win32::Security::GetTokenInformation;
use windows_sys::Win32::Security::{
    DACL_SECURITY_INFORMATION, NO_INHERITANCE, PROTECTED_DACL_SECURITY_INFORMATION,
    SUB_CONTAINERS_AND_OBJECTS_INHERIT, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle, LOCKFILE_EXCLUSIVE_LOCK, LockFileEx,
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW, UnlockFileEx,
};
use windows_sys::Win32::System::IO::OVERLAPPED;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

pub(super) struct DescriptorLock {
    file: File,
    overlapped: OVERLAPPED,
}

pub(super) struct PinnedRuntimeDir(PathBuf);

#[derive(Eq, PartialEq)]
pub(super) struct OwnedFileIdentity {
    volume: u32,
    index: u64,
}

impl PinnedRuntimeDir {
    pub(super) fn open(path: &Path) -> io::Result<Self> {
        if !path.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "AgentRun runtime directory is unavailable",
            ));
        }
        Ok(Self(path.to_path_buf()))
    }

    pub(super) fn lock_private_descriptor(&self) -> io::Result<DescriptorLock> {
        lock_private_descriptor(&self.0)
    }

    pub(super) fn create_private_file(&self, name: &str) -> io::Result<File> {
        create_private_file(&self.0.join(name))
    }

    pub(super) fn read_private_file(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.ensure_parent(path)?;
        read_private_file(path)
    }

    pub(super) fn replace_private_descriptor(
        &self,
        temporary_name: &str,
        path: &Path,
    ) -> io::Result<()> {
        replace_private_descriptor(&self.0.join(temporary_name), path)
    }

    pub(super) fn secure_published_descriptor(&self, path: &Path) -> io::Result<()> {
        self.ensure_parent(path)?;
        secure_published_descriptor(path)
    }

    pub(super) fn remove_file(&self, name: &str) -> io::Result<()> {
        fs::remove_file(self.0.join(name))
    }

    pub(super) fn remove_path(&self, path: &Path) -> io::Result<()> {
        self.ensure_parent(path)?;
        fs::remove_file(path)
    }

    pub(super) fn remove_owned_file(
        &self,
        name: &str,
        identity: &OwnedFileIdentity,
    ) -> io::Result<()> {
        let path = self.0.join(name);
        let file = File::open(&path)?;
        if file_identity(&file)? != *identity {
            return Err(io::Error::other(
                "AgentRun descriptor identity changed before cleanup",
            ));
        }
        drop(file);
        fs::remove_file(path)
    }

    pub(super) fn remove_owned_path(
        &self,
        path: &Path,
        identity: &OwnedFileIdentity,
    ) -> io::Result<()> {
        self.ensure_parent(path)?;
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| io::Error::other("AgentRun descriptor name is not UTF-8"))?;
        self.remove_owned_file(name, identity)
    }

    pub(super) fn sync(&self) -> io::Result<()> {
        let _ = fs::metadata(&self.0)?;
        Ok(())
    }

    fn ensure_parent(&self, path: &Path) -> io::Result<()> {
        if path.parent() != Some(self.0.as_path()) {
            return Err(io::Error::other(
                "AgentRun descriptor escaped its runtime directory",
            ));
        }
        Ok(())
    }
}

#[allow(unsafe_code)]
pub(super) fn file_identity(file: &File) -> io::Result<OwnedFileIdentity> {
    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: `information` is writable for the duration of the call and the
    // borrowed file keeps its valid handle alive.
    if unsafe {
        GetFileInformationByHandle(file.as_raw_handle().cast(), ptr::addr_of_mut!(information))
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(OwnedFileIdentity {
        volume: information.dwVolumeSerialNumber,
        index: (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow),
    })
}

#[allow(unsafe_code)]
impl Drop for DescriptorLock {
    fn drop(&mut self) {
        // SAFETY: the file handle and OVERLAPPED remain alive for the lock's
        // entire lifetime and identify the exact byte range locked below.
        let _ = unsafe {
            UnlockFileEx(
                self.file.as_raw_handle().cast(),
                0,
                1,
                0,
                ptr::addr_of_mut!(self.overlapped),
            )
        };
    }
}

pub(super) fn prepare_private_runtime_dir(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    protect_current_user(path, SUB_CONTAINERS_AND_OBJECTS_INHERIT)
}

pub(super) fn create_private_file(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new().write(true).create_new(true).open(path)?;
    if let Err(error) = protect_current_user(path, NO_INHERITANCE) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(error);
    }
    Ok(file)
}

#[allow(unsafe_code)]
pub(super) fn lock_private_descriptor(runtime_dir: &Path) -> io::Result<DescriptorLock> {
    let path = runtime_dir.join(".agent-registry.lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)?;
    protect_current_user(&path, NO_INHERITANCE)?;
    let mut overlapped = OVERLAPPED::default();
    // SAFETY: the owned file and stack OVERLAPPED outlive the call and are
    // retained unchanged in DescriptorLock until the matching unlock.
    if unsafe {
        LockFileEx(
            file.as_raw_handle().cast(),
            LOCKFILE_EXCLUSIVE_LOCK,
            0,
            1,
            0,
            ptr::addr_of_mut!(overlapped),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(DescriptorLock { file, overlapped })
}

#[allow(unsafe_code)]
pub(super) fn replace_private_descriptor(temporary: &Path, path: &Path) -> io::Result<()> {
    let temporary = wide_path(temporary);
    let path = wide_path(path);
    // SAFETY: both UTF-16 buffers are NUL-terminated and remain alive for the
    // duration of the synchronous MoveFileExW call.
    if unsafe {
        MoveFileExW(
            temporary.as_ptr(),
            path.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(super) fn secure_published_descriptor(path: &Path) -> io::Result<()> {
    protect_current_user(path, NO_INHERITANCE)
}

pub(super) fn read_private_file(path: &Path) -> io::Result<Vec<u8>> {
    let mut contents = Vec::new();
    File::open(path)?.read_to_end(&mut contents)?;
    Ok(contents)
}

#[allow(unsafe_code, clippy::too_many_lines)]
fn protect_current_user(path: &Path, inheritance: u32) -> io::Result<()> {
    let mut token: HANDLE = ptr::null_mut();
    // SAFETY: token points to writable storage and receives one owned handle.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, ptr::addr_of_mut!(token)) } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let result = (|| {
        let mut required = 0_u32;
        // SAFETY: the documented sizing call accepts a null output buffer.
        let _ = unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                ptr::null_mut(),
                0,
                ptr::addr_of_mut!(required),
            )
        };
        if required == 0 {
            return Err(io::Error::last_os_error());
        }
        let words = usize::try_from(required)
            .map_err(|_| io::Error::other("token information is too large"))?
            .div_ceil(std::mem::size_of::<usize>());
        let mut buffer = vec![0_usize; words];
        // SAFETY: the aligned buffer has at least `required` writable bytes.
        if unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                required,
                ptr::addr_of_mut!(required),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful TokenUser output begins with TOKEN_USER and its
        // SID pointer remains valid while `buffer` is alive.
        let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
        if user.User.Sid.is_null() {
            return Err(io::Error::other("process token has no user SID"));
        }
        let trustee = TRUSTEE_W {
            pMultipleTrustee: ptr::null_mut(),
            MultipleTrusteeOperation: 0,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_USER,
            ptstrName: user.User.Sid.cast(),
        };
        let access = EXPLICIT_ACCESS_W {
            grfAccessPermissions: GENERIC_ALL,
            grfAccessMode: SET_ACCESS,
            grfInheritance: inheritance,
            Trustee: trustee,
        };
        let mut acl = ptr::null_mut();
        // SAFETY: access and acl are valid for the synchronous API call.
        let status = unsafe {
            SetEntriesInAclW(
                1,
                ptr::addr_of!(access),
                ptr::null(),
                ptr::addr_of_mut!(acl),
            )
        };
        if status != 0 {
            return Err(win32_error(status));
        }
        if acl.is_null() {
            return Err(io::Error::other("private AgentRun ACL is empty"));
        }
        let wide = wide_path(path);
        // SAFETY: path is NUL-terminated and acl is the allocation returned by
        // SetEntriesInAclW. The call does not retain either pointer.
        let status = unsafe {
            SetNamedSecurityInfoW(
                wide.as_ptr().cast_mut(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                acl,
                ptr::null(),
            )
        };
        // SAFETY: acl was allocated by LocalAlloc inside SetEntriesInAclW and
        // is released exactly once after its last use.
        let _ = unsafe { LocalFree(acl.cast::<c_void>()) };
        if status != 0 {
            return Err(win32_error(status));
        }
        Ok(())
    })();
    // SAFETY: token is the non-null owned handle returned above.
    let _ = unsafe { CloseHandle(token) };
    result
}

fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn win32_error(status: u32) -> io::Error {
    i32::try_from(status).map_or_else(
        |_| io::Error::other(format!("Win32 error {status}")),
        io::Error::from_raw_os_error,
    )
}
