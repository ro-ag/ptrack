#![cfg(windows)]
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::fs::File;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::path::Path;
use std::ptr;

use windows_sys::Wdk::Storage::FileSystem::{
    FILE_RENAME_INFORMATION, FileRenameInformation, NtSetInformationFile,
};
use windows_sys::Win32::Foundation::RtlNtStatusToDosError;
use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_ALL, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    EXPLICIT_ACCESS_W, GetNamedSecurityInfoW, GetSecurityInfo, SE_FILE_OBJECT, SET_ACCESS,
    SetEntriesInAclW, SetNamedSecurityInfoW, SetSecurityInfo, TRUSTEE_IS_SID, TRUSTEE_IS_USER,
    TRUSTEE_W,
};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACL, ACL_SIZE_INFORMATION, AclSizeInformation, DACL_SECURITY_INFORMATION,
    EqualSid, GetAce, GetAclInformation, GetTokenInformation, NO_INHERITANCE,
    OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
    SUB_CONTAINERS_AND_OBJECTS_INHERIT, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CreateFileW, DELETE, FILE_ALL_ACCESS, FILE_ATTRIBUTE_DIRECTORY,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_DISPOSITION_INFO, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, FileDispositionInfo, GetFileInformationByHandle,
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW, OPEN_EXISTING,
    SetFileInformationByHandle, WRITE_DAC, WRITE_OWNER,
};
use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;
use windows_sys::Win32::System::SystemServices::ACCESS_ALLOWED_ACE_TYPE;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct WindowsFileIdentity {
    pub volume: u32,
    pub index: u64,
}

pub(crate) fn open_no_reparse(
    path: &Path,
    directory: bool,
    writable: bool,
    exclusive: bool,
) -> io::Result<File> {
    let share = if exclusive {
        0
    } else {
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE
    };
    open_no_reparse_with_share(path, directory, writable, share)
}

pub(crate) fn open_no_reparse_no_delete(
    path: &Path,
    directory: bool,
    writable: bool,
) -> io::Result<File> {
    open_no_reparse_with_share(
        path,
        directory,
        writable,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
    )
}

pub(crate) fn open_staging_directory_for_publish(path: &Path) -> io::Result<File> {
    open_no_reparse_with_access(
        path,
        true,
        FILE_GENERIC_READ | FILE_GENERIC_WRITE | WRITE_DAC | WRITE_OWNER | DELETE,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
    )
}

pub(crate) fn delete_directory_handle(directory: &File) -> io::Result<()> {
    let mut information = FILE_DISPOSITION_INFO { DeleteFile: true };
    // SAFETY: directory owns a live handle with DELETE access and information
    // is retained for the duration of the synchronous call.
    if unsafe {
        SetFileInformationByHandle(
            directory.as_raw_handle().cast(),
            FileDispositionInfo,
            ptr::addr_of_mut!(information).cast(),
            u32::try_from(std::mem::size_of_val(&information))
                .expect("FILE_DISPOSITION_INFO size fits u32"),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(crate) fn rename_directory_handle_no_replace(
    directory: &File,
    root: &File,
    name: &str,
) -> io::Result<()> {
    let name = name.encode_utf16().collect::<Vec<_>>();
    let name_bytes = name
        .len()
        .checked_mul(std::mem::size_of::<u16>())
        .ok_or_else(|| io::Error::other("name too long"))?;
    let bytes = std::mem::size_of::<FILE_RENAME_INFORMATION>()
        .checked_add(name_bytes)
        .ok_or_else(|| io::Error::other("name too long"))?;
    let units = bytes.div_ceil(std::mem::size_of::<usize>());
    let mut storage = vec![0_usize; units];
    let information = storage.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
    // SAFETY: storage is pointer-aligned and sized for FILE_RENAME_INFORMATION plus
    // the exact UTF-16 filename payload consumed synchronously by the kernel.
    unsafe {
        (*information).Anonymous.ReplaceIfExists = false;
        (*information).RootDirectory = root.as_raw_handle().cast();
        (*information).FileNameLength =
            u32::try_from(name_bytes).map_err(|_| io::Error::other("name too long"))?;
        ptr::copy_nonoverlapping(
            name.as_ptr(),
            ptr::addr_of_mut!((*information).FileName).cast::<u16>(),
            name.len(),
        );
        let mut status = IO_STATUS_BLOCK::default();
        let result = NtSetInformationFile(
            directory.as_raw_handle().cast(),
            &mut status,
            information.cast_const().cast(),
            u32::try_from(bytes).map_err(|_| io::Error::other("name too long"))?,
            FileRenameInformation,
        );
        if result != 0 {
            return Err(io::Error::from_raw_os_error(
                RtlNtStatusToDosError(result) as i32
            ));
        }
    }
    Ok(())
}

pub(crate) fn random_staging_bytes(output: &mut [u8]) -> io::Result<()> {
    #[link(name = "advapi32")]
    unsafe extern "system" {
        #[link_name = "SystemFunction036"]
        fn rtl_gen_random(buffer: *mut c_void, length: u32) -> u8;
    }
    let length = u32::try_from(output.len()).map_err(|_| io::Error::other("buffer too large"))?;
    // SAFETY: output is a live writable buffer retained for the synchronous call.
    if unsafe { rtl_gen_random(output.as_mut_ptr().cast(), length) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn open_no_reparse_with_share(
    path: &Path,
    directory: bool,
    writable: bool,
    share: u32,
) -> io::Result<File> {
    let access = if writable {
        FILE_GENERIC_READ | FILE_GENERIC_WRITE
    } else {
        FILE_GENERIC_READ
    };
    open_no_reparse_with_access(path, directory, access, share)
}

fn open_no_reparse_with_access(
    path: &Path,
    directory: bool,
    access: u32,
    share: u32,
) -> io::Result<File> {
    let wide = wide_path(path);
    let flags = FILE_FLAG_OPEN_REPARSE_POINT
        | if directory {
            FILE_FLAG_BACKUP_SEMANTICS
        } else {
            0
        };
    // SAFETY: path is NUL-terminated; the returned owned handle is converted
    // exactly once into File on success.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            access,
            share,
            ptr::null(),
            OPEN_EXISTING,
            flags,
            ptr::null_mut(),
        )
    };
    if handle.is_null() || handle == (-1_isize as HANDLE) {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: handle ownership transfers to File.
    let file = unsafe { File::from_raw_handle(handle) };
    let information = information(&file)?;
    let attributes = information.dwFileAttributes;
    if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || directory != (attributes & FILE_ATTRIBUTE_DIRECTORY != 0)
    {
        return Err(io::Error::other(
            "path is a reparse point or has the wrong type",
        ));
    }
    Ok(file)
}

pub(crate) fn identity(file: &File) -> io::Result<WindowsFileIdentity> {
    let information = information(file)?;
    Ok(WindowsFileIdentity {
        volume: information.dwVolumeSerialNumber,
        index: (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow),
    })
}

pub(crate) fn protect_file(path: &Path) -> io::Result<()> {
    protect_current_user(path, NO_INHERITANCE)
}

pub(crate) fn protect_handle(file: &File) -> io::Result<()> {
    protect_handle_with_inheritance(file, NO_INHERITANCE)
}

pub(crate) fn protect_directory_handle(file: &File) -> io::Result<()> {
    protect_handle_with_inheritance(file, SUB_CONTAINERS_AND_OBJECTS_INHERIT)
}

fn protect_handle_with_inheritance(file: &File, inheritance: u32) -> io::Result<()> {
    let user = current_user()?;
    let acl = current_user_acl(user.sid, inheritance)?;
    // SAFETY: file owns a live handle with WRITE_DAC and WRITE_OWNER; the SID
    // and ACL remain valid for the duration of the synchronous call.
    let status = unsafe {
        SetSecurityInfo(
            file.as_raw_handle().cast(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION
                | DACL_SECURITY_INFORMATION
                | PROTECTED_DACL_SECURITY_INFORMATION,
            user.sid,
            ptr::null_mut(),
            acl,
            ptr::null_mut(),
        )
    };
    // SAFETY: ACL ownership came from SetEntriesInAclW.
    let _ = unsafe { LocalFree(acl.cast::<c_void>()) };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    Ok(())
}

pub(crate) fn protect_directory(path: &Path) -> io::Result<()> {
    protect_current_user(path, SUB_CONTAINERS_AND_OBJECTS_INHERIT)
}

pub(crate) fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    let source = wide_path(source);
    let destination = wide_path(destination);
    // SAFETY: both paths are NUL-terminated and retained for the duration of
    // the synchronous call. The flags request same-volume replacement and a
    // durable metadata flush before success is reported.
    if unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(crate) fn verify_private(path: &Path) -> io::Result<()> {
    let user = current_user()?;
    let wide = wide_path(path);
    let mut dacl = ptr::null_mut::<ACL>();
    let mut owner = ptr::null_mut();
    let mut descriptor = ptr::null_mut();
    // SAFETY: writable outputs receive borrowed pointers into the returned
    // descriptor, whose allocation is retained until all checks finish.
    let status = unsafe {
        GetNamedSecurityInfoW(
            wide.as_ptr().cast_mut(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            &mut dacl,
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 || descriptor.is_null() || dacl.is_null() {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    let result = verify_owner_dacl(owner, dacl, user.sid);
    // SAFETY: descriptor ownership came from GetNamedSecurityInfoW.
    let _ = unsafe { LocalFree(descriptor.cast::<c_void>()) };
    result
}

pub(crate) fn verify_private_handle(file: &File) -> io::Result<()> {
    let user = current_user()?;
    let mut dacl = ptr::null_mut::<ACL>();
    let mut owner = ptr::null_mut();
    let mut descriptor = ptr::null_mut();
    // SAFETY: the file owns a live handle and all requested outputs remain
    // valid until the returned descriptor is released.
    let status = unsafe {
        GetSecurityInfo(
            file.as_raw_handle().cast(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            &mut dacl,
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 || descriptor.is_null() || dacl.is_null() {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    let result = verify_owner_dacl(owner, dacl, user.sid);
    // SAFETY: descriptor ownership came from GetSecurityInfo.
    let _ = unsafe { LocalFree(descriptor.cast::<c_void>()) };
    result
}

fn verify_owner_dacl(owner: *mut c_void, dacl: *mut ACL, user_sid: *mut c_void) -> io::Result<()> {
    if owner.is_null() || unsafe { EqualSid(owner, user_sid) } == 0 {
        return Err(io::Error::other(
            "private path owner is not the current user",
        ));
    }
    let mut information = ACL_SIZE_INFORMATION::default();
    // SAFETY: dacl and output structure are valid for the synchronous call.
    if unsafe {
        GetAclInformation(
            dacl,
            ptr::addr_of_mut!(information).cast(),
            u32::try_from(std::mem::size_of_val(&information))
                .map_err(|_| io::Error::other("ACL size overflow"))?,
            AclSizeInformation,
        )
    } == 0
        || information.AceCount == 0
        || information.AceCount > 8
    {
        return Err(io::Error::other("private DACL has an invalid ACE count"));
    }
    for index in 0..information.AceCount {
        let mut ace = ptr::null_mut();
        // SAFETY: the index is bounded by the queried ACE count.
        if unsafe { GetAce(dacl, index, &mut ace) } == 0 || ace.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: GetAce returned a pointer to an ACE; the header type proves
        // ACCESS_ALLOWED_ACE layout, whose SidStart is the variable SID start.
        let allowed = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
        if u32::from(allowed.Header.AceType) != ACCESS_ALLOWED_ACE_TYPE {
            return Err(io::Error::other("private DACL contains a non-allow ACE"));
        }
        // SetEntriesInAclW may preserve the generic bit or expand it to the
        // object-specific full-control mask. Both encode the same authority.
        if allowed.Mask != GENERIC_ALL && allowed.Mask != FILE_ALL_ACCESS {
            return Err(io::Error::other(
                "private DACL does not grant exact owner authority",
            ));
        }
        let sid = ptr::addr_of!(allowed.SidStart).cast_mut().cast();
        // SAFETY: both SIDs are valid for the duration of the comparison.
        if unsafe { EqualSid(sid, user_sid) } == 0 {
            return Err(io::Error::other("private DACL belongs to another identity"));
        }
    }
    Ok(())
}

fn information(file: &File) -> io::Result<BY_HANDLE_FILE_INFORMATION> {
    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: file owns a live handle and output is writable.
    if unsafe {
        GetFileInformationByHandle(file.as_raw_handle().cast(), ptr::addr_of_mut!(information))
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(information)
}

struct CurrentUser {
    token: HANDLE,
    buffer: Vec<usize>,
    sid: *mut c_void,
}

impl Drop for CurrentUser {
    fn drop(&mut self) {
        let _ = self.buffer.len();
        // SAFETY: token is the owned handle returned by OpenProcessToken.
        let _ = unsafe { CloseHandle(self.token) };
    }
}

fn current_user() -> io::Result<CurrentUser> {
    let mut token: HANDLE = ptr::null_mut();
    // SAFETY: token is a writable output for the current process token.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let result = (|| {
        let mut required = 0_u32;
        // SAFETY: documented sizing query accepts null output.
        let _ = unsafe { GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut required) };
        let words = usize::try_from(required)
            .map_err(|_| io::Error::other("token size overflow"))?
            .div_ceil(std::mem::size_of::<usize>());
        if words == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut buffer = vec![0_usize; words];
        // SAFETY: aligned buffer contains the queried writable byte count.
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
        // SAFETY: successful TokenUser output begins with TOKEN_USER.
        let sid = unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid };
        if sid.is_null() {
            return Err(io::Error::other("current user SID is unavailable"));
        }
        Ok(CurrentUser { token, buffer, sid })
    })();
    if result.is_err() {
        // SAFETY: token was opened above and is not transferred on error.
        let _ = unsafe { CloseHandle(token) };
    }
    result
}

fn protect_current_user(path: &Path, inheritance: u32) -> io::Result<()> {
    let user = current_user()?;
    let acl = current_user_acl(user.sid, inheritance)?;
    let wide = wide_path(path);
    // SAFETY: path is NUL-terminated; ACL remains allocated for the call.
    let status = unsafe {
        SetNamedSecurityInfoW(
            wide.as_ptr().cast_mut(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION
                | DACL_SECURITY_INFORMATION
                | PROTECTED_DACL_SECURITY_INFORMATION,
            user.sid,
            ptr::null_mut(),
            acl,
            ptr::null(),
        )
    };
    // SAFETY: ACL ownership came from SetEntriesInAclW.
    let _ = unsafe { LocalFree(acl.cast::<c_void>()) };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    Ok(())
}

fn current_user_acl(user_sid: *mut c_void, inheritance: u32) -> io::Result<*mut ACL> {
    let trustee = TRUSTEE_W {
        pMultipleTrustee: ptr::null_mut(),
        MultipleTrusteeOperation: 0,
        TrusteeForm: TRUSTEE_IS_SID,
        TrusteeType: TRUSTEE_IS_USER,
        ptstrName: user_sid.cast(),
    };
    let access = EXPLICIT_ACCESS_W {
        grfAccessPermissions: GENERIC_ALL,
        grfAccessMode: SET_ACCESS,
        grfInheritance: inheritance,
        Trustee: trustee,
    };
    let mut acl = ptr::null_mut();
    // SAFETY: access and ACL output are valid for the call.
    let status = unsafe { SetEntriesInAclW(1, &access, ptr::null(), &mut acl) };
    if status != 0 || acl.is_null() {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    Ok(acl)
}

fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}
