#![cfg(windows)]
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::fs::File;
use std::io::{Read, Write};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::path::{Component, Path};
use std::ptr;

use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows_sys::Wdk::Storage::FileSystem::{
    FILE_CREATE, FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_REPARSE_POINT,
    FILE_RENAME_POSIX_SEMANTICS, FILE_RENAME_REPLACE_IF_EXISTS, FILE_SYNCHRONOUS_IO_NONALERT,
    FileDispositionInformation, FileRenameInformation, NtCreateFile, NtSetInformationFile,
};
use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_ALL, INVALID_HANDLE_VALUE, LocalFree};
use windows_sys::Win32::Foundation::{OBJ_DONT_REPARSE, STATUS_SUCCESS, UNICODE_STRING};
use windows_sys::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_TABLE2};
use windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows_sys::Win32::Security::Authorization::{
    EXPLICIT_ACCESS_W, SE_FILE_OBJECT, SET_ACCESS, SetEntriesInAclW, SetNamedSecurityInfoW,
    TRUSTEE_IS_SID, TRUSTEE_IS_USER, TRUSTEE_W,
};
use windows_sys::Win32::Security::{
    DACL_SECURITY_INFORMATION, GetTokenInformation, PROTECTED_DACL_SECURITY_INFORMATION,
    TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CreateFileW, DELETE, FILE_ATTRIBUTE_DIRECTORY,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TEMPORARY, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    FlushFileBuffers, GetFileInformationByHandle, OPEN_EXISTING, SYNCHRONIZE,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, OpenProcessToken, OpenThread, ResumeThread, THREAD_SUSPEND_RESUME,
};

use crate::git::GitError;

pub(super) struct ProcessJob(windows_sys::Win32::Foundation::HANDLE);

impl ProcessJob {
    pub(super) fn attach(child: &tokio::process::Child) -> Result<Self, ()> {
        contain_suspended(&WindowsSpawnApi { child })
    }

    pub(super) fn terminate(&self) {
        // SAFETY: self owns a live job handle; the exit code is diagnostic-only.
        let _ = unsafe { TerminateJobObject(self.0, 1) };
    }
}

pub(crate) trait SuspendedSpawnApi {
    type Job;

    fn create_kill_on_close_job(&self) -> Result<Self::Job, ()>;
    fn assign_suspended_process(&self, job: &Self::Job) -> Result<(), ()>;
    fn resume_primary_thread(&self) -> Result<(), ()>;
}

pub(crate) fn contain_suspended<A: SuspendedSpawnApi>(api: &A) -> Result<A::Job, ()> {
    let job = api.create_kill_on_close_job()?;
    api.assign_suspended_process(&job)?;
    api.resume_primary_thread()?;
    Ok(job)
}

struct WindowsSpawnApi<'a> {
    child: &'a tokio::process::Child,
}

impl SuspendedSpawnApi for WindowsSpawnApi<'_> {
    type Job = ProcessJob;

    fn create_kill_on_close_job(&self) -> Result<Self::Job, ()> {
        // SAFETY: null security/name creates one unnamed job with default ACL.
        let job = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if job.is_null() {
            return Err(());
        }
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: job is owned and limits points to a correctly sized value.
        let configured = unsafe {
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                ptr::addr_of!(limits).cast(),
                u32::try_from(std::mem::size_of_val(&limits)).map_err(|_| ())?,
            )
        };
        if configured == 0 {
            // SAFETY: job is the owned handle created above.
            let _ = unsafe { CloseHandle(job) };
            return Err(());
        }
        Ok(ProcessJob(job))
    }

    fn assign_suspended_process(&self, job: &Self::Job) -> Result<(), ()> {
        let process = self.child.raw_handle().ok_or(())?;
        // SAFETY: both handles remain valid for the duration of the call.
        if unsafe { AssignProcessToJobObject(job.0, process.cast()) } == 0 {
            return Err(());
        }
        Ok(())
    }

    fn resume_primary_thread(&self) -> Result<(), ()> {
        let process_id = self.child.id().ok_or(())?;
        resume_only_process_thread(process_id)
    }
}

fn resume_only_process_thread(process_id: u32) -> Result<(), ()> {
    // SAFETY: the snapshot call takes values and returns one owned handle.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(());
    }
    let result = (|| {
        let mut entry = THREADENTRY32 {
            dwSize: u32::try_from(std::mem::size_of::<THREADENTRY32>()).map_err(|_| ())?,
            ..THREADENTRY32::default()
        };
        let mut thread_id = None;
        // SAFETY: snapshot is valid and entry points to its declared size.
        let mut available = unsafe { Thread32First(snapshot, &mut entry) } != 0;
        while available {
            if entry.th32OwnerProcessID == process_id {
                if thread_id.replace(entry.th32ThreadID).is_some() {
                    return Err(());
                }
            }
            // SAFETY: snapshot and entry remain valid through enumeration.
            available = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
        }
        let thread_id = thread_id.ok_or(())?;
        // SAFETY: requested rights are limited to resuming the identified
        // thread and the returned handle is owned by this scope.
        let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, thread_id) };
        if thread.is_null() {
            return Err(());
        }
        // SAFETY: thread is the unique primary thread of the still-suspended
        // child, established by the snapshot enumeration above.
        let previous = unsafe { ResumeThread(thread) };
        // SAFETY: thread is the owned handle returned by OpenThread.
        let _ = unsafe { CloseHandle(thread) };
        // CREATE_SUSPENDED establishes a suspend count of exactly one. Any
        // other value means the child was not in the state this containment
        // protocol requires; closing the job then fails closed.
        if previous != 1 {
            return Err(());
        }
        Ok(())
    })();
    // SAFETY: snapshot is the owned handle returned by the snapshot call.
    let _ = unsafe { CloseHandle(snapshot) };
    result
}

impl Drop for ProcessJob {
    fn drop(&mut self) {
        // SAFETY: self owns the job handle exactly once. KILL_ON_JOB_CLOSE
        // terminates any descendant which survived normal completion.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

pub(super) fn private_windows_acl(path: &Path) -> Result<(), GitError> {
    let failure = || GitError::internal("temporary path could not be protected");
    let mut token = ptr::null_mut();
    // SAFETY: valid process pseudo-handle and writable token output.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(failure());
    }
    let result = (|| {
        let mut required = 0_u32;
        // SAFETY: the first call queries the required TokenUser buffer size.
        let _ = unsafe { GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut required) };
        let words = usize::try_from(required)
            .unwrap_or_default()
            .div_ceil(std::mem::size_of::<usize>());
        if words == 0 {
            return Err(failure());
        }
        let mut buffer = vec![0_usize; words];
        // SAFETY: aligned buffer contains at least the required writable bytes.
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
            return Err(failure());
        }
        // SAFETY: successful TokenUser output begins with TOKEN_USER and the
        // SID remains valid for the lifetime of buffer.
        let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
        if user.User.Sid.is_null() {
            return Err(failure());
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
            grfInheritance: 0,
            Trustee: trustee,
        };
        let mut acl = ptr::null_mut();
        // SAFETY: access and ACL output are valid for this synchronous call.
        let status = unsafe { SetEntriesInAclW(1, &access, ptr::null(), &mut acl) };
        if status != 0 || acl.is_null() {
            return Err(failure());
        }
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: path is NUL-terminated and ACL remains valid for the call.
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
        // SAFETY: ACL ownership came from SetEntriesInAclW.
        let _ = unsafe { LocalFree(acl.cast::<c_void>()) };
        if status != 0 {
            return Err(failure());
        }
        Ok(())
    })();
    // SAFETY: token is the owned handle returned by OpenProcessToken.
    let _ = unsafe { CloseHandle(token) };
    result
}

pub(super) fn protect_private_path(path: &Path) -> Result<(), ()> {
    private_windows_acl(path).map_err(|_| ())
}

pub(super) fn active_interface_names() -> Result<Vec<String>, ()> {
    let mut table = ptr::null_mut::<MIB_IF_TABLE2>();
    // SAFETY: table is a writable output pointer owned by this scope.
    if unsafe { GetIfTable2(&mut table) } != 0 || table.is_null() {
        return Err(());
    }
    let result = (|| {
        // SAFETY: a successful GetIfTable2 returns a table with NumEntries
        // contiguous rows beginning at Table.
        let count = unsafe { (*table).NumEntries } as usize;
        // SAFETY: the API contract establishes exactly count initialized rows.
        let rows = unsafe { std::slice::from_raw_parts((*table).Table.as_ptr(), count) };
        let names = rows
            .iter()
            .filter(|row| row.OperStatus == IfOperStatusUp)
            .filter_map(|row| {
                let end = row.Alias.iter().position(|value| *value == 0)?;
                String::from_utf16(&row.Alias[..end]).ok()
            })
            .collect();
        Ok(names)
    })();
    // SAFETY: table was allocated by GetIfTable2 and is released once.
    unsafe { FreeMibTable(table.cast()) };
    result
}

pub(super) fn install_download(
    project: &Path,
    destination: &Path,
    staged: &Path,
    maximum: i64,
) -> Result<(), &'static str> {
    let relative = destination
        .strip_prefix(project)
        .map_err(|_| "capability denied: download destination escapes the project")?;
    let components = relative
        .components()
        .map(|component| match component {
            Component::Normal(value) => Ok(value),
            _ => Err("capability denied: download destination escapes the project"),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let (final_name, ancestors) = components
        .split_last()
        .ok_or("capability denied: download destination escapes the project")?;
    let mut directories = Vec::with_capacity(ancestors.len() + 1);
    directories.push(open_project_directory(
        ptr::null_mut(),
        &windows_nt_path(project),
    )?);
    for component in ancestors {
        let parent = directories.last().ok_or(
            "capability denied: download destination parent is not a stable project directory",
        )?;
        directories.push(open_project_directory(parent.0, component)?);
    }
    let parent = directories.last().ok_or(
        "capability denied: download destination parent is not a stable project directory",
    )?;
    let source_path: Vec<u16> = staged
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: source_path is NUL-terminated and arguments request a no-follow
    // read handle with no inherited security attributes.
    let source_handle = unsafe {
        CreateFileW(
            source_path.as_ptr(),
            FILE_GENERIC_READ,
            FILE_SHARE_READ,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT,
            ptr::null_mut(),
        )
    };
    if source_handle == INVALID_HANDLE_VALUE {
        return Err("capability denied: download staging file is invalid");
    }
    // SAFETY: source_handle is newly owned and transferred exactly once.
    let mut source = unsafe { File::from_raw_handle(source_handle.cast()) };
    let source_info = by_handle_information(source.as_raw_handle().cast())?;
    if source_info.dwFileAttributes & (FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_DIRECTORY) != 0
    {
        return Err("capability denied: download staging file is invalid");
    }
    let temporary_name = format!(".ptrack-download-{}", random_hex_windows(16)?);
    let temporary_handle = create_project_file(parent.0, &temporary_name)?;
    // SAFETY: temporary_handle is newly owned and transferred exactly once.
    let mut temporary = unsafe { File::from_raw_handle(temporary_handle.cast()) };
    let allowed = u64::try_from(maximum).unwrap_or_default();
    let install = (|| {
        let copied = std::io::copy(
            &mut Read::by_ref(&mut source).take(allowed.saturating_add(1)),
            &mut temporary,
        )
        .map_err(|_| "download install failed")?;
        if copied > allowed {
            return Err("HTTP response exceeds its byte limit");
        }
        temporary.flush().map_err(|_| "download install failed")?;
        // SAFETY: the file owns a valid writable handle.
        if unsafe { FlushFileBuffers(temporary.as_raw_handle().cast()) } == 0 {
            return Err("download install failed");
        }
        rename_project_file(temporary.as_raw_handle().cast(), parent.0, final_name)
    })();
    if install.is_err() {
        mark_file_for_deletion(temporary.as_raw_handle().cast());
    }
    install
}

struct OwnedHandle(windows_sys::Win32::Foundation::HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: the handle is owned exactly once by this wrapper.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

fn open_project_directory(
    root: windows_sys::Win32::Foundation::HANDLE,
    name: &std::ffi::OsStr,
) -> Result<OwnedHandle, &'static str> {
    let unicode = NtUnicode::new(name)?;
    let attributes = unicode.attributes(root);
    let mut handle = ptr::null_mut();
    let mut status = IO_STATUS_BLOCK::default();
    // SAFETY: all pointer arguments remain live through this synchronous call.
    let result = unsafe {
        NtCreateFile(
            &mut handle,
            FILE_GENERIC_READ | SYNCHRONIZE,
            &attributes,
            &mut status,
            ptr::null(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            FILE_OPEN,
            FILE_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
            ptr::null(),
            0,
        )
    };
    if result != STATUS_SUCCESS || handle.is_null() {
        return Err(
            "capability denied: download destination parent is not a stable project directory",
        );
    }
    let owned = OwnedHandle(handle);
    let information = by_handle_information(handle)?;
    if information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err("capability denied: download destination parent contains a reparse point");
    }
    Ok(owned)
}

fn create_project_file(
    parent: windows_sys::Win32::Foundation::HANDLE,
    name: &str,
) -> Result<windows_sys::Win32::Foundation::HANDLE, &'static str> {
    let unicode = NtUnicode::new(std::ffi::OsStr::new(name))?;
    let attributes = unicode.attributes(parent);
    let mut handle = ptr::null_mut();
    let mut status = IO_STATUS_BLOCK::default();
    // SAFETY: all pointer arguments remain live through this synchronous call.
    let result = unsafe {
        NtCreateFile(
            &mut handle,
            FILE_GENERIC_READ | FILE_GENERIC_WRITE | DELETE | SYNCHRONIZE,
            &attributes,
            &mut status,
            ptr::null(),
            FILE_ATTRIBUTE_TEMPORARY,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            FILE_CREATE,
            FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
            ptr::null(),
            0,
        )
    };
    if result != STATUS_SUCCESS || handle.is_null() {
        return Err("download install failed");
    }
    Ok(handle)
}

fn by_handle_information(
    handle: windows_sys::Win32::Foundation::HANDLE,
) -> Result<BY_HANDLE_FILE_INFORMATION, &'static str> {
    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: information is a correctly sized writable output.
    if unsafe { GetFileInformationByHandle(handle, &mut information) } == 0 {
        return Err("capability denied: download staging file is invalid");
    }
    Ok(information)
}

#[repr(C)]
pub(crate) struct RenameLayout {
    flags: u32,
    root: windows_sys::Win32::Foundation::HANDLE,
    length: u32,
    name: [u16; 1],
}

fn rename_project_file(
    handle: windows_sys::Win32::Foundation::HANDLE,
    parent: windows_sys::Win32::Foundation::HANDLE,
    name: &std::ffi::OsStr,
) -> Result<(), &'static str> {
    let name: Vec<u16> = name.encode_wide().collect();
    let name_bytes = name.len().checked_mul(2).ok_or("download install failed")?;
    let offset = std::mem::offset_of!(RenameLayout, name);
    let mut buffer = vec![0_u8; rename_buffer_len(name_bytes)?];
    let header = RenameLayout {
        flags: FILE_RENAME_REPLACE_IF_EXISTS | FILE_RENAME_POSIX_SEMANTICS,
        root: parent,
        length: u32::try_from(name_bytes).map_err(|_| "download install failed")?,
        name: [0],
    };
    // SAFETY: buffer has storage for the complete padded header and UTF-16 name.
    unsafe {
        ptr::write_unaligned(buffer.as_mut_ptr().cast::<RenameLayout>(), header);
        ptr::copy_nonoverlapping(
            name.as_ptr().cast::<u8>(),
            buffer.as_mut_ptr().add(offset),
            name_bytes,
        );
    }
    let mut status = IO_STATUS_BLOCK::default();
    // SAFETY: buffer and status remain valid for the synchronous API call.
    if unsafe {
        NtSetInformationFile(
            handle,
            &mut status,
            buffer.as_ptr().cast(),
            u32::try_from(buffer.len()).map_err(|_| "download install failed")?,
            FileRenameInformation,
        )
    } != STATUS_SUCCESS
    {
        return Err("download install failed");
    }
    Ok(())
}

pub(crate) fn rename_buffer_len(name_bytes: usize) -> Result<usize, &'static str> {
    std::mem::offset_of!(RenameLayout, name)
        .checked_add(name_bytes)
        .map(|length| length.max(std::mem::size_of::<RenameLayout>()))
        .ok_or("download install failed")
}

fn mark_file_for_deletion(handle: windows_sys::Win32::Foundation::HANDLE) {
    let delete = true;
    let mut status = IO_STATUS_BLOCK::default();
    // SAFETY: delete and status remain live for this synchronous best-effort call.
    let _ = unsafe {
        NtSetInformationFile(
            handle,
            &mut status,
            ptr::addr_of!(delete).cast(),
            u32::try_from(std::mem::size_of_val(&delete)).unwrap_or_default(),
            FileDispositionInformation,
        )
    };
}

struct NtUnicode {
    words: Vec<u16>,
    value: UNICODE_STRING,
}

impl NtUnicode {
    fn new(value: &std::ffi::OsStr) -> Result<Self, &'static str> {
        let mut words: Vec<u16> = value.encode_wide().collect();
        let length = words
            .len()
            .checked_mul(2)
            .ok_or("download install failed")?;
        words.push(0);
        let mut result = Self {
            value: UNICODE_STRING {
                Length: u16::try_from(length).map_err(|_| "download install failed")?,
                MaximumLength: u16::try_from(length + 2).map_err(|_| "download install failed")?,
                Buffer: ptr::null_mut(),
            },
            words,
        };
        result.value.Buffer = result.words.as_mut_ptr();
        Ok(result)
    }

    fn attributes(&self, root: windows_sys::Win32::Foundation::HANDLE) -> OBJECT_ATTRIBUTES {
        OBJECT_ATTRIBUTES {
            Length: u32::try_from(std::mem::size_of::<OBJECT_ATTRIBUTES>()).unwrap_or_default(),
            RootDirectory: root,
            ObjectName: &self.value,
            Attributes: OBJ_DONT_REPARSE,
            SecurityDescriptor: ptr::null(),
            SecurityQualityOfService: ptr::null(),
        }
    }
}

fn windows_nt_path(path: &Path) -> std::ffi::OsString {
    let value = path.as_os_str().to_string_lossy();
    if let Some(unc) = value.strip_prefix(r"\\") {
        std::ffi::OsString::from(format!(r"\??\UNC\{unc}"))
    } else {
        std::ffi::OsString::from(format!(r"\??\{value}"))
    }
}

fn random_hex_windows(length: usize) -> Result<String, &'static str> {
    let mut bytes = vec![0_u8; length];
    getrandom::fill(&mut bytes).map_err(|_| "download install failed")?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}
