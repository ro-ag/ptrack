/// Reports whether `pid` currently names a live process.
///
/// This is only a fast staleness hint: PID reuse means it is never identity or
/// authority evidence.
#[must_use]
#[cfg(unix)]
pub fn process_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    let Some(process) = rustix::process::Pid::from_raw(pid) else {
        return false;
    };
    match rustix::process::test_kill_process(process) {
        Ok(()) => true,
        Err(error) => error == rustix::io::Errno::PERM,
    }
}

#[must_use]
#[cfg(windows)]
#[allow(unsafe_code)]
pub fn process_alive(pid: i32) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    const STILL_ACTIVE: u32 = 259;
    if pid <= 0 {
        return false;
    }
    // SAFETY: OpenProcess receives a positive PID and no inherited handle. The
    // returned handle is checked and closed exactly once before returning.
    let Ok(pid) = u32::try_from(pid) else {
        return false;
    };
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return false;
    }
    let mut code = 0;
    // SAFETY: `handle` is valid above and `code` points to initialized writable
    // memory for the duration of the call.
    let queried = unsafe { GetExitCodeProcess(handle, std::ptr::addr_of_mut!(code)) } != 0;
    // SAFETY: `handle` is the non-null owned handle returned by OpenProcess and
    // has not previously been closed.
    let _ = unsafe { CloseHandle(handle) };
    queried && code == STILL_ACTIVE
}
