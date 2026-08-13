#![cfg(windows)]
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr;

use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_ALL, INVALID_HANDLE_VALUE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    EXPLICIT_ACCESS_W, SE_FILE_OBJECT, SET_ACCESS, SetEntriesInAclW, SetNamedSecurityInfoW,
    TRUSTEE_IS_SID, TRUSTEE_IS_USER, TRUSTEE_W,
};
use windows_sys::Win32::Security::{
    DACL_SECURITY_INFORMATION, GetTokenInformation, PROTECTED_DACL_SECURITY_INFORMATION,
    TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
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
