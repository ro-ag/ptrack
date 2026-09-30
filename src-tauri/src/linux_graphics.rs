//! `WebKitGTK` can fail to present DMA-BUF frames on NVIDIA/hybrid desktops.
//! Keep the workaround local to GUI launches and respect explicit overrides.
//! <https://v2.tauri.app/develop/debug/linux-graphics/>

use std::ffi::OsStr;
use std::os::unix::process::CommandExt;
use std::process::Command;

const DISABLE_DMABUF: &str = "WEBKIT_DISABLE_DMABUF_RENDERER";

/// Called first in `main`, before GTK initialization or application workers.
/// Xlib is also used by GTK on `XWayland`; no display connection is opened here.
#[allow(unsafe_code)] // The only FFI call, before any other Xlib use in this process.
pub fn initialize_xlib() -> std::io::Result<()> {
    let xlib = x11_dl::xlib::Xlib::open().map_err(std::io::Error::other)?;
    // SAFETY: main calls this before any application/thread/GTK initialization.
    // XInitThreads takes no pointers, and x11-dl retains its library handle
    // for the process lifetime. A renderer-workaround exec starts main again.
    if unsafe { (xlib.XInitThreads)() } == 0 {
        return Err(std::io::Error::other("XInitThreads failed"));
    }
    Ok(())
}

fn restart_command(
    executable: &OsStr,
    arguments: impl IntoIterator<Item = impl AsRef<OsStr>>,
    nvidia_loaded: bool,
    override_value: Option<&OsStr>,
) -> Option<Command> {
    if !nvidia_loaded || override_value.is_some() {
        return None;
    }
    let mut command = Command::new(executable);
    command.args(arguments).env(DISABLE_DMABUF, "1");
    Some(command)
}

pub fn prepare() -> std::io::Result<()> {
    let override_value = std::env::var_os(DISABLE_DMABUF);
    let nvidia_loaded = std::path::Path::new("/sys/module/nvidia").exists();
    if !nvidia_loaded || override_value.is_some() {
        return Ok(());
    }
    let executable = std::env::current_exe()?;
    if let Some(mut command) = restart_command(
        executable.as_os_str(),
        std::env::args_os().skip(1),
        nvidia_loaded,
        override_value.as_deref(),
    ) {
        // Command::env configures the replacement process without unsafe
        // mutation of this process's environment. exec preserves PID, cwd,
        // arguments and inherited environment; the override prevents a loop.
        return Err(command.exec());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_preserves_arguments_and_sets_child_environment() {
        let arguments = ["gui", "/tmp/project with spaces", "--plan", "7"];
        let command = restart_command(OsStr::new("/tmp/p-track"), arguments, true, None)
            .expect("NVIDIA without an override needs the workaround");
        assert_eq!(command.get_program(), OsStr::new("/tmp/p-track"));
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            arguments.map(OsStr::new)
        );
        assert_eq!(
            command.get_envs().collect::<Vec<_>>(),
            vec![(OsStr::new(DISABLE_DMABUF), Some(OsStr::new("1")))]
        );
    }

    #[test]
    fn explicit_values_are_preserved_and_replacement_does_not_loop() {
        for value in ["0", "1", ""] {
            assert!(
                restart_command(OsStr::new("ptrack"), ["gui"], true, Some(OsStr::new(value)),)
                    .is_none()
            );
        }
    }

    #[test]
    fn other_drivers_keep_the_default_renderer() {
        assert!(restart_command(OsStr::new("ptrack"), ["gui"], false, None).is_none());
    }
}
