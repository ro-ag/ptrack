use std::collections::BTreeMap;
#[cfg(unix)]
use std::fs;
#[cfg(unix)]
use std::path::PathBuf;
#[cfg(unix)]
use std::process::Command;

use crate::profile::{CwdPolicy, ExitBehavior, Profile, ProfileKind};
use crate::shell_integration::{
    ShellIntegrationDescriptor, ShellIntegrationOwner, ShellIntegrationQuality,
};
#[cfg(unix)]
use crate::shell_integration::{prepare_shell_integration, supports_shell_integration};

fn profile(kind: ProfileKind, executable: &str, args: &[&str]) -> Profile {
    Profile {
        id: "shell".to_owned(),
        name: "Shell".to_owned(),
        kind,
        provider: String::new(),
        executable: executable.to_owned(),
        args: args.iter().map(|argument| (*argument).to_owned()).collect(),
        env: BTreeMap::new(),
        theme: String::new(),
        font_family: String::new(),
        font_size: 0,
        scrollback: 0,
        cwd_policy: CwdPolicy::Requested,
        fixed_cwd: String::new(),
        exit_behavior: ExitBehavior::Keep,
    }
}

#[test]
fn descriptor_json_is_exact_and_omits_empty_nonce() {
    assert_eq!(
        serde_json::to_string(&ShellIntegrationDescriptor::none()).unwrap(),
        r#"{"quality":"none"}"#
    );
    let rich = ShellIntegrationDescriptor {
        quality: ShellIntegrationQuality::Rich,
        nonce: "per-session-correlation".to_owned(),
    };
    assert_eq!(
        serde_json::to_string(&rich).unwrap(),
        r#"{"quality":"rich","nonce":"per-session-correlation"}"#
    );
    assert!(
        serde_json::from_str::<ShellIntegrationDescriptor>(
            r#"{"quality":"rich","nonce":"n","extra":true}"#
        )
        .is_err()
    );
}

#[test]
fn windows_shell_integration_is_unavailable() {
    let zsh = profile(ProfileKind::Shell, "/bin/zsh", &["-l"]);
    assert!(
        ShellIntegrationOwner::new_for_os([&zsh], "windows")
            .unwrap()
            .is_none()
    );
}

#[cfg(unix)]
#[test]
fn agent_and_command_launches_are_never_mutated() {
    let owner = shell_owner();
    let cases = [
        profile(ProfileKind::Agent, "/bin/zsh", &["-i"]),
        profile(ProfileKind::Shell, "/bin/zsh", &["-c", "echo hidden"]),
        profile(ProfileKind::Shell, "/bin/bash", &["-l"]),
        profile(ProfileKind::Shell, "/bin/bash", &["script.sh"]),
        profile(ProfileKind::Shell, "/bin/fish", &["-i"]),
    ];
    for value in cases {
        let environment = vec!["HOME=/users/test".to_owned()];
        let (args, env, descriptor) = owner.prepare(&value, &environment, "nonce");
        assert_eq!(descriptor, ShellIntegrationDescriptor::none());
        assert_eq!(args, value.args);
        assert_eq!(env, environment);
    }
    owner.close().unwrap();
}

#[cfg(unix)]
#[test]
fn owner_creates_private_bounded_hooks_and_removes_them_only_when_closed() {
    let owner = shell_owner();
    let directory = owner.directory().to_owned();
    let entries = fs::read_dir(&directory)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(entries.len(), 6);
    for entry in entries {
        let metadata = entry.metadata().unwrap();
        assert!(metadata.len() > 0 && metadata.len() <= 16 * 1_024);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    let shell = profile(ProfileKind::Shell, "/bin/zsh", &["-i"]);
    let (_, _, descriptor) = owner.prepare(&shell, &[], "first-session");
    assert_eq!(descriptor.quality, ShellIntegrationQuality::Rich);
    assert!(directory.exists(), "per-session preparation removed hooks");
    owner.close().unwrap();
    assert!(!directory.exists());
}

#[cfg(unix)]
#[test]
fn zsh_integration_preserves_args_sources_startup_and_layers_zdotdir() {
    let owner = shell_owner();
    let value = profile(ProfileKind::Shell, "/bin/zsh", &["-li"]);
    let original_args = value.args.clone();
    let environment = vec![
        "HOME=/users/test".to_owned(),
        "ZDOTDIR=/users/test/config/zsh".to_owned(),
        "PATH=/bin".to_owned(),
    ];
    let original_environment = environment.clone();
    let (args, prepared, descriptor) = owner.prepare(&value, &environment, "nonce-value");
    assert_eq!(args, ["-li"]);
    assert_eq!(
        environment_value(&prepared, "ZDOTDIR"),
        owner.directory().to_string_lossy()
    );
    assert_eq!(
        environment_value(&prepared, "PTRACK_SHELL_ORIGINAL_ZDOTDIR_V1"),
        "/users/test/config/zsh"
    );
    assert_eq!(
        environment_value(&prepared, "PTRACK_SHELL_INTEGRATION_WRAPPER_V1"),
        owner.directory().to_string_lossy()
    );
    assert_eq!(
        environment_value(&prepared, "PTRACK_SHELL_INTEGRATION_NONCE_V1"),
        "nonce-value"
    );
    assert_eq!(
        descriptor,
        ShellIntegrationDescriptor {
            quality: ShellIntegrationQuality::Rich,
            nonce: "nonce-value".to_owned(),
        }
    );
    assert_eq!(value.args, original_args);
    assert_eq!(environment, original_environment);

    let zshrc = fs::read_to_string(owner.directory().join(".zshrc")).unwrap();
    assert!(
        zshrc
            .find(r#"source "${PTRACK_SHELL_ORIGINAL_ZDOTDIR_V1}/.zshrc""#)
            .unwrap()
            < zshrc.find("add-zsh-hook precmd").unwrap()
    );
    assert!(zshrc.contains("export ZDOTDIR=\"${PTRACK_SHELL_ORIGINAL_ZDOTDIR_V1}\""));
    assert_authenticated_lifecycle(&zshrc);
    owner.close().unwrap();
}

#[cfg(unix)]
#[test]
fn zsh_uses_home_when_zdotdir_is_absent() {
    let owner = shell_owner();
    let value = profile(ProfileKind::Shell, "/bin/zsh", &[]);
    let (_, prepared, descriptor) =
        owner.prepare(&value, &["HOME=/users/test".to_owned()], "nonce-value");
    assert_eq!(descriptor.quality, ShellIntegrationQuality::Rich);
    assert_eq!(
        environment_value(&prepared, "PTRACK_SHELL_ORIGINAL_ZDOTDIR_V1"),
        "/users/test"
    );
    owner.close().unwrap();
}

#[cfg(unix)]
#[test]
fn bash_integration_uses_private_init_file_and_preserves_user_hooks() {
    let owner = shell_owner();
    let value = profile(ProfileKind::Shell, "/bin/bash", &["-i"]);
    let environment = vec![
        "HOME=/users/test".to_owned(),
        "PROMPT_COMMAND=user_prompt".to_owned(),
    ];
    let (args, prepared, descriptor) = owner.prepare(&value, &environment, "nonce-value");
    assert_eq!(args[0], "--init-file");
    assert_eq!(
        PathBuf::from(&args[1]),
        owner.directory().join("bash-regular")
    );
    assert_eq!(args[2], "-i");
    assert_eq!(
        environment_value(&prepared, "PTRACK_SHELL_INTEGRATION_NONCE_V1"),
        "nonce-value"
    );
    assert_eq!(descriptor.quality, ShellIntegrationQuality::Rich);

    let contents = fs::read_to_string(&args[1]).unwrap();
    assert!(contents.contains("~/.bashrc"));
    assert!(contents.contains("__ptrack_original_prompt_command"));
    assert!(contents.contains("__ptrack_original_debug_trap"));
    assert_authenticated_lifecycle(&contents);
    if Command::new("bash").arg("--version").output().is_ok() {
        assert!(
            Command::new("bash")
                .arg("-n")
                .arg(&args[1])
                .status()
                .unwrap()
                .success()
        );
    }
    owner.close().unwrap();
}

#[cfg(unix)]
#[test]
fn bash_login_shell_and_absent_owner_keep_launch_semantics() {
    let owner = shell_owner();
    for flags in [&["-l"][..], &["--login", "-i"][..]] {
        let value = profile(ProfileKind::Shell, "/bin/bash", flags);
        let environment = vec!["HOME=/users/test".to_owned()];
        let (args, env, descriptor) = owner.prepare(&value, &environment, "nonce");
        assert_eq!(descriptor, ShellIntegrationDescriptor::none());
        assert_eq!(args, value.args);
        assert_eq!(env, environment);
    }

    let value = profile(ProfileKind::Shell, "/bin/zsh", &["-i"]);
    let environment = vec!["HOME=/users/test".to_owned()];
    let (args, env, descriptor) = prepare_shell_integration(None, &value, &environment, "unused");
    assert_eq!(descriptor, ShellIntegrationDescriptor::none());
    assert_eq!(args, value.args);
    assert_eq!(env, environment);
    owner.close().unwrap();
}

#[cfg(unix)]
#[test]
fn supported_flags_are_purely_interactive_combinations() {
    for flags in [
        &[][..],
        &["-i"][..],
        &["-l"][..],
        &["-li"][..],
        &["--login", "--interactive"][..],
    ] {
        let zsh = profile(ProfileKind::Shell, "/bin/zsh", flags);
        assert!(supports_shell_integration(&zsh));
    }
    for flags in [
        &["-c", "pwd"][..],
        &["--"][..],
        &["---"][..],
        &["--li"][..],
        &["-x"][..],
    ] {
        let zsh = profile(ProfileKind::Shell, "/bin/zsh", flags);
        assert!(!supports_shell_integration(&zsh));
    }
    let bash_login = profile(ProfileKind::Shell, "/bin/bash", &["-li"]);
    assert!(!supports_shell_integration(&bash_login));
}

#[cfg(unix)]
fn assert_authenticated_lifecycle(contents: &str) {
    for marker in [
        "133;A",
        "133;B",
        "133;C",
        "133;D",
        "633;A",
        "633;B",
        "633;C",
        "633;D",
        "]7;file://",
        "633;P;Cwd=file://",
        "unset PTRACK_SHELL_INTEGRATION_NONCE_V1",
        "[A-Za-z0-9/._~-]",
        "<= 4000",
    ] {
        assert!(contents.contains(marker), "missing hook marker {marker:?}");
    }
    assert!(!contents.contains("633;E"), "hook leaked command text");
}

#[cfg(unix)]
fn shell_owner() -> ShellIntegrationOwner {
    let zsh = profile(ProfileKind::Shell, "/bin/zsh", &[]);
    let bash = profile(ProfileKind::Shell, "/bin/bash", &["-i"]);
    ShellIntegrationOwner::new([&zsh, &bash])
        .unwrap()
        .expect("supported shells should create an owner")
}

#[cfg(unix)]
fn environment_value(environment: &[String], key: &str) -> String {
    environment
        .iter()
        .find_map(|entry| {
            let (name, value) = entry.split_once('=')?;
            (name == key).then(|| value.to_owned())
        })
        .unwrap_or_default()
}
