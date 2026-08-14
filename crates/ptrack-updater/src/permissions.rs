use std::fs::File;
use std::io;
use std::path::Path;

#[cfg(unix)]
mod platform {
    use std::fs;
    use std::os::unix::fs::DirBuilderExt;
    use std::os::unix::fs::PermissionsExt;

    use rustix::fs::{Mode, OFlags};

    use super::{File, Path, io};

    pub(crate) fn prepare_private_dir(path: &Path) -> io::Result<()> {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true).mode(0o700).create(path)?;
        secure_private_path(path, true)
    }

    pub(crate) fn create_private_dir(path: &Path) -> io::Result<()> {
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700).create(path)?;
        secure_private_path(path, true)
    }

    pub(crate) fn secure_private_path(path: &Path, directory: bool) -> io::Result<()> {
        let metadata = fs::symlink_metadata(path)?;
        validate_type(&metadata, directory)?;
        let mode = if directory { 0o700 } else { 0o600 };
        fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
        validate_private_path(path, directory)
    }

    pub(crate) fn validate_private_path(path: &Path, directory: bool) -> io::Result<()> {
        let metadata = fs::symlink_metadata(path)?;
        validate_type(&metadata, directory)?;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(io::Error::other("update path is not private"));
        }
        Ok(())
    }

    pub(crate) fn open_private_regular(path: &Path) -> io::Result<File> {
        let descriptor = rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )?;
        let file = File::from(descriptor);
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.permissions().mode() & 0o077 != 0 {
            return Err(io::Error::other(
                "update file is not a private regular file",
            ));
        }
        Ok(file)
    }

    pub(crate) fn create_private_regular(path: &Path) -> io::Result<File> {
        let descriptor = rustix::fs::open(
            path,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::RUSR | Mode::WUSR,
        )?;
        rustix::fs::fchmod(&descriptor, Mode::RUSR | Mode::WUSR)?;
        Ok(File::from(descriptor))
    }

    fn validate_type(metadata: &fs::Metadata, directory: bool) -> io::Result<()> {
        let kind = metadata.file_type();
        if kind.is_symlink() || (directory && !kind.is_dir()) || (!directory && !kind.is_file()) {
            return Err(io::Error::other("update path has an unsafe type"));
        }
        Ok(())
    }
}

#[cfg(windows)]
mod platform {
    #![allow(unsafe_code)]

    use std::ffi::c_void;
    use std::fs;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::FromRawHandle;
    use std::ptr;

    use windows_sys::Win32::Foundation::{
        CloseHandle, GENERIC_ALL, HANDLE, INVALID_HANDLE_VALUE, LocalFree,
    };
    use windows_sys::Win32::Security::Authorization::{
        EXPLICIT_ACCESS_W, SE_FILE_OBJECT, SET_ACCESS, SetEntriesInAclW, SetSecurityInfo,
        TRUSTEE_IS_SID, TRUSTEE_IS_USER, TRUSTEE_W,
    };
    use windows_sys::Win32::Security::{
        DACL_SECURITY_INFORMATION, GetTokenInformation, PROTECTED_DACL_SECURITY_INFORMATION,
        SUB_CONTAINERS_AND_OBJECTS_INHERIT, TOKEN_QUERY, TOKEN_USER, TokenUser,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_DIRECTORY,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_READ, GetFileInformationByHandle,
        OPEN_EXISTING, WRITE_DAC,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    use super::{File, Path, io};

    pub(crate) fn prepare_private_dir(path: &Path) -> io::Result<()> {
        fs::create_dir_all(path)?;
        secure_private_path(path, true)
    }

    pub(crate) fn create_private_dir(path: &Path) -> io::Result<()> {
        fs::create_dir(path)?;
        secure_private_path(path, true)
    }

    pub(crate) fn secure_private_path(path: &Path, directory: bool) -> io::Result<()> {
        let handle = open_handle(path, directory, FILE_GENERIC_READ | WRITE_DAC)?;
        let result = protect_current_user(handle, directory);
        unsafe { CloseHandle(handle) };
        result
    }

    pub(crate) fn validate_private_path(path: &Path, directory: bool) -> io::Result<()> {
        secure_private_path(path, directory)
    }

    pub(crate) fn open_private_regular(path: &Path) -> io::Result<File> {
        let handle = open_handle(path, false, FILE_GENERIC_READ | WRITE_DAC)?;
        if let Err(error) = protect_current_user(handle, false) {
            unsafe { CloseHandle(handle) };
            return Err(error);
        }
        Ok(unsafe { File::from_raw_handle(handle.cast()) })
    }

    pub(crate) fn create_private_regular(path: &Path) -> io::Result<File> {
        let wide = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                FILE_GENERIC_READ | FILE_GENERIC_WRITE | WRITE_DAC,
                FILE_SHARE_READ,
                ptr::null(),
                CREATE_NEW,
                FILE_FLAG_OPEN_REPARSE_POINT,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        if let Err(error) = protect_current_user(handle, false) {
            unsafe { CloseHandle(handle) };
            return Err(error);
        }
        Ok(unsafe { File::from_raw_handle(handle.cast()) })
    }

    fn open_handle(path: &Path, directory: bool, access: u32) -> io::Result<HANDLE> {
        let wide = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let flags = FILE_FLAG_OPEN_REPARSE_POINT
            | if directory {
                FILE_FLAG_BACKUP_SEMANTICS
            } else {
                0
            };
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                access,
                FILE_SHARE_READ,
                ptr::null(),
                OPEN_EXISTING,
                flags,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let mut information = BY_HANDLE_FILE_INFORMATION::default();
        if unsafe { GetFileInformationByHandle(handle, &mut information) } == 0 {
            let error = io::Error::last_os_error();
            unsafe { CloseHandle(handle) };
            return Err(error);
        }
        let is_directory = information.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0;
        if information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || is_directory != directory
        {
            unsafe { CloseHandle(handle) };
            return Err(io::Error::other("update path has an unsafe type"));
        }
        Ok(handle)
    }

    fn protect_current_user(handle: HANDLE, directory: bool) -> io::Result<()> {
        let mut token = ptr::null_mut();
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let result = (|| {
            let mut required = 0;
            unsafe { GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut required) };
            let words = usize::try_from(required)
                .unwrap_or_default()
                .div_ceil(std::mem::size_of::<usize>());
            if words == 0 {
                return Err(io::Error::last_os_error());
            }
            let mut buffer = vec![0_usize; words];
            if unsafe {
                GetTokenInformation(
                    token,
                    TokenUser,
                    buffer.as_mut_ptr().cast(),
                    required,
                    &mut required,
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
            let access = EXPLICIT_ACCESS_W {
                grfAccessPermissions: GENERIC_ALL,
                grfAccessMode: SET_ACCESS,
                grfInheritance: if directory {
                    SUB_CONTAINERS_AND_OBJECTS_INHERIT
                } else {
                    0
                },
                Trustee: TRUSTEE_W {
                    pMultipleTrustee: ptr::null_mut(),
                    MultipleTrusteeOperation: 0,
                    TrusteeForm: TRUSTEE_IS_SID,
                    TrusteeType: TRUSTEE_IS_USER,
                    ptstrName: user.User.Sid.cast(),
                },
            };
            let mut acl = ptr::null_mut();
            let status = unsafe { SetEntriesInAclW(1, &access, ptr::null(), &mut acl) };
            if status != 0 || acl.is_null() {
                return Err(io::Error::from_raw_os_error(status.cast_signed()));
            }
            let status = unsafe {
                SetSecurityInfo(
                    handle,
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    acl,
                    ptr::null_mut(),
                )
            };
            unsafe { LocalFree(acl.cast::<c_void>()) };
            if status != 0 {
                return Err(io::Error::from_raw_os_error(status.cast_signed()));
            }
            Ok(())
        })();
        unsafe { CloseHandle(token) };
        result
    }
}

pub(crate) use platform::{
    create_private_dir, create_private_regular, open_private_regular, prepare_private_dir,
    secure_private_path, validate_private_path,
};
