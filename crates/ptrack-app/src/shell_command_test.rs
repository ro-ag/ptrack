#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;

#[cfg(unix)]
use super::shell_command::{
    SHELL_PATH_MARKER_BEGIN, SHELL_PATH_MARKER_END, ShellPathChange, ensure_shell_path,
    install_shell_command_from,
};

#[cfg(unix)]
#[test]
fn shell_path_block_is_exact_private_enough_and_idempotent() {
    let directory = tempfile_directory("exact");
    let profile = directory.join(".zprofile");
    let executable = directory.join("p-track.app/Contents/MacOS/ptrack");
    std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
    std::fs::write(&profile, b"export EDITOR=vim\n# no trailing newline:").unwrap();

    let message = install_shell_command_from(&executable, &directory).unwrap();
    let bin_dir = executable.parent().unwrap();
    assert_eq!(
        message,
        format!(
            "Added to PATH in {}:\n\n{}\n\nOpen a new terminal window, then run `ptrack`.",
            profile.display(),
            bin_dir.display()
        )
    );
    let content = std::fs::read_to_string(&profile).unwrap();
    assert_eq!(
        content,
        format!(
            "export EDITOR=vim\n# no trailing newline:\n{SHELL_PATH_MARKER_BEGIN}\n\
             # Added by p-track: makes the `ptrack` CLI available in new terminal sessions.\n\
             export PATH=\"$PATH:{}\"\n{SHELL_PATH_MARKER_END}\n",
            bin_dir.display()
        )
    );
    #[cfg(unix)]
    assert_eq!(
        std::fs::metadata(&profile).unwrap().permissions().mode() & 0o777,
        0o644
    );

    let before = std::fs::read(&profile).unwrap();
    assert_eq!(
        ensure_shell_path(&profile, bin_dir).unwrap(),
        ShellPathChange::Unchanged
    );
    assert_eq!(std::fs::read(&profile).unwrap(), before);
    assert_eq!(
        install_shell_command_from(&executable, &directory).unwrap(),
        format!(
            "Already on PATH via {}:\n\n{}",
            profile.display(),
            bin_dir.display()
        )
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[test]
fn missing_profile_is_created_with_the_exact_managed_block() {
    let directory = tempfile_directory("missing");
    let profile = directory.join(".zprofile");
    assert_eq!(
        ensure_shell_path(
            &profile,
            std::path::Path::new("/Applications/p-track.app/Contents/MacOS")
        )
        .unwrap(),
        ShellPathChange::Added
    );
    assert_eq!(
        std::fs::read_to_string(profile).unwrap(),
        concat!(
            "# >>> ptrack cli >>>\n",
            "# Added by p-track: makes the `ptrack` CLI available in new terminal sessions.\n",
            "export PATH=\"$PATH:/Applications/p-track.app/Contents/MacOS\"\n",
            "# <<< ptrack cli <<<\n"
        )
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[test]
fn existing_managed_block_is_idempotent_without_write_access() {
    let directory = tempfile_directory("read-only");
    let profile = directory.join(".zprofile");
    let block = format!(
        "{SHELL_PATH_MARKER_BEGIN}\n\
         # Added by p-track: makes the `ptrack` CLI available in new terminal sessions.\n\
         export PATH=\"$PATH:/safe\"\n{SHELL_PATH_MARKER_END}\n"
    );
    std::fs::write(&profile, &block).unwrap();
    let mut permissions = std::fs::metadata(&profile).unwrap().permissions();
    permissions.set_mode(0o444);
    std::fs::set_permissions(&profile, permissions).unwrap();

    assert_eq!(
        ensure_shell_path(&profile, std::path::Path::new("/safe")).unwrap(),
        ShellPathChange::Unchanged
    );
    assert_eq!(std::fs::read_to_string(&profile).unwrap(), block);
    std::fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[test]
fn a_block_for_another_install_location_is_replaced_and_reported() {
    let directory = tempfile_directory("stale");
    let profile = directory.join(".zprofile");
    let old = std::path::Path::new("/Volumes/old/p-track.app/Contents/MacOS");
    let new = directory.join("p-track.app/Contents/MacOS");
    std::fs::create_dir_all(&new).unwrap();
    std::fs::write(
        &profile,
        b"export EDITOR=vim
",
    )
    .unwrap();
    assert_eq!(
        ensure_shell_path(&profile, old).unwrap(),
        ShellPathChange::Added
    );
    std::fs::OpenOptions::new()
        .append(true)
        .open(&profile)
        .and_then(|mut file| {
            std::io::Write::write_all(
                &mut file,
                b"alias ll='ls -l'
",
            )
        })
        .unwrap();
    std::fs::set_permissions(&profile, std::fs::Permissions::from_mode(0o600)).unwrap();

    let message = install_shell_command_from(&new.join("ptrack"), &directory).unwrap();
    assert!(message.starts_with("Updated PATH in "), "{message}");
    let content = std::fs::read_to_string(&profile).unwrap();
    assert_eq!(
        content,
        format!(
            "export EDITOR=vim\n{SHELL_PATH_MARKER_BEGIN}\n\
             # Added by p-track: makes the `ptrack` CLI available in new terminal sessions.\n\
             export PATH=\"$PATH:{}\"\n{SHELL_PATH_MARKER_END}\nalias ll='ls -l'\n",
            new.display()
        )
    );
    assert_eq!(content.matches(SHELL_PATH_MARKER_BEGIN).count(), 1);
    assert_eq!(
        std::fs::metadata(&profile).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(
        install_shell_command_from(&new.join("ptrack"), &directory)
            .unwrap()
            .starts_with("Already on PATH via ")
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[test]
fn shell_profile_symlinks_and_shell_active_executable_paths_fail_closed() {
    use std::os::unix::fs::symlink;

    let directory = tempfile_directory("unsafe");
    let target = directory.join("target");
    std::fs::write(&target, b"untouched").unwrap();
    symlink(&target, directory.join(".zprofile")).unwrap();
    let error =
        ensure_shell_path(&directory.join(".zprofile"), std::path::Path::new("/safe")).unwrap_err();
    assert!(error.starts_with("cannot update "));
    assert_eq!(std::fs::read(target).unwrap(), b"untouched");

    let executable = directory.join("unsafe$path/ptrack");
    assert_eq!(
        install_shell_command_from(&executable, &directory).unwrap_err(),
        "cannot update shell PATH: executable directory is unsafe"
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
fn tempfile_directory(label: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "ptrack-shell-command-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&path).unwrap();
    path
}
