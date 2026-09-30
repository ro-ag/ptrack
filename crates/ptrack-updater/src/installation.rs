use std::path::Path;

use crate::Installation;

pub(crate) fn linux_host_installation() -> Installation {
    let Ok(executable) = std::env::current_exe() else {
        // Unknown ownership must never enable an in-place replacement.
        return Installation::LinuxPackageManager;
    };
    let app_dir = std::env::var_os("APPDIR").and_then(|path| std::fs::canonicalize(path).ok());
    classify(
        &executable,
        app_dir.as_deref(),
        Path::new("/usr/lib/ptrack/package-manager").is_file(),
    )
}

fn classify(executable: &Path, app_dir: Option<&Path>, managed_marker: bool) -> Installation {
    if executable.starts_with("/nix/store") {
        return Installation::LinuxPackageManager;
    }
    if app_dir.is_some_and(|root| executable == root.join("usr/bin/ptrack")) {
        return Installation::LinuxAppImage;
    }
    if managed_marker && executable == Path::new("/usr/bin/ptrack") {
        return Installation::LinuxPackageManager;
    }
    Installation::Archive
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appimage_environment_must_describe_the_running_executable() {
        let root = Path::new("/tmp/.mount_ptrack");
        assert_eq!(
            classify(&root.join("usr/bin/ptrack"), Some(root), false),
            Installation::LinuxAppImage
        );
        assert_eq!(
            classify(Path::new("/home/user/bin/ptrack"), Some(root), false),
            Installation::Archive
        );
    }

    #[test]
    fn system_marker_does_not_capture_a_separate_user_archive() {
        assert_eq!(
            classify(Path::new("/usr/bin/ptrack"), None, true),
            Installation::LinuxPackageManager
        );
        assert_eq!(
            classify(Path::new("/home/user/bin/ptrack"), None, true),
            Installation::Archive
        );
        assert_eq!(
            classify(Path::new("/nix/store/abc-ptrack/bin/ptrack"), None, false),
            Installation::LinuxPackageManager
        );
    }
}
