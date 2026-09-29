//! Recognizes a copy installed by the per-user Windows installer.
#![allow(unsafe_code)]

use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};

/// The value the MSI records as its HKCU key path (`build/windows/p-track.wxs`).
const INSTALL_KEY: &str = r"Software\ro-ag\p-track";
const INSTALL_VALUE: &str = "InstallDir";
/// Longest recorded folder accepted, in UTF-16 units including the NUL.
const MAX_INSTALL_DIR_UNITS: usize = 32_768;

/// True only when the running executable sits in the folder the installer
/// recorded. A portable or archive copy elsewhere keeps archive updates even
/// while an installed copy also exists.
pub(crate) fn running_from_installed_copy() -> bool {
    let Some(recorded) = recorded_install_dir() else {
        return false;
    };
    std::env::current_exe()
        .ok()
        .is_some_and(|executable| same_directory(&recorded, executable.parent()))
}

pub(crate) fn same_directory(recorded: &Path, running: Option<&Path>) -> bool {
    match (
        std::fs::canonicalize(recorded),
        running.map(std::fs::canonicalize),
    ) {
        (Ok(recorded), Some(Ok(running))) => recorded == running,
        _ => false,
    }
}

fn recorded_install_dir() -> Option<PathBuf> {
    let wide = |text: &str| {
        std::ffi::OsStr::new(text)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>()
    };
    let key = wide(INSTALL_KEY);
    let value = wide(INSTALL_VALUE);
    let mut buffer = vec![0_u16; MAX_INSTALL_DIR_UNITS];
    let mut bytes = u32::try_from(buffer.len() * 2).ok()?;
    // SAFETY: key and value are NUL-terminated; the output buffer holds
    // `bytes` writable bytes, and RegGetValueW writes the length it used.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &raw mut bytes,
        )
    };
    if status != 0 {
        return None;
    }
    let units = (usize::try_from(bytes).ok()? / 2).min(buffer.len());
    let text = &buffer[..units];
    let end = text
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(text.len());
    if end == 0 {
        return None;
    }
    Some(PathBuf::from(OsString::from_wide(&text[..end])))
}
