use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::profile::{
    CwdPolicy, DEFAULT_PROFILE_FONT_FAMILY, DEFAULT_PROFILE_FONT_SIZE, DEFAULT_PROFILE_SCROLLBACK,
    DEFAULT_PROFILE_THEME, ExitBehavior, MAX_PROFILE_FONT_SIZE, Profile, ProfileKind,
    build_environment_for_os, discover_profiles_with, profile_executable_is_available, resolve_cwd,
    sort_profiles, validate_profile_with,
};

fn profile(id: &str, kind: ProfileKind, executable: &Path) -> Profile {
    Profile {
        id: id.to_owned(),
        name: id.to_owned(),
        kind,
        provider: String::new(),
        executable: executable.to_string_lossy().into_owned(),
        args: Vec::new(),
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
fn validation_normalizes_defaults_provider_and_owns_a_deep_copy() {
    let directory = test_directory("profile-defaults");
    let mut input = profile("agent-codex", ProfileKind::Agent, &directory.join("codex"));
    input.args = vec!["--mode".to_owned()];
    input.env.insert("PAGER".to_owned(), "less".to_owned());

    let validated = validate_profile_with(&input, |_| unreachable!()).unwrap();
    assert_eq!(validated.provider, "codex");
    assert_eq!(validated.theme, DEFAULT_PROFILE_THEME);
    assert_eq!(validated.font_family, DEFAULT_PROFILE_FONT_FAMILY);
    assert_eq!(validated.font_size, DEFAULT_PROFILE_FONT_SIZE);
    assert_eq!(validated.scrollback, DEFAULT_PROFILE_SCROLLBACK);
    assert_eq!(validated.cwd_policy, CwdPolicy::Requested);
    assert_eq!(validated.exit_behavior, ExitBehavior::Keep);

    input.args[0] = "changed".to_owned();
    input.env.insert("PAGER".to_owned(), "changed".to_owned());
    assert_eq!(validated.args, ["--mode"]);
    assert_eq!(validated.env["PAGER"], "less");
    remove_test_directory(&directory);
}

#[test]
fn validation_enforces_identity_size_policy_and_environment_bounds() {
    let directory = test_directory("profile-bounds");
    let executable = directory.join("shell");
    let mut valid = profile("shell-profile", ProfileKind::Shell, &executable);
    valid.theme = "solarized-dark".to_owned();
    valid.font_family = "Iosevka, monospace".to_owned();
    valid.font_size = 10;
    valid.scrollback = 100_000;
    valid.cwd_policy = CwdPolicy::Fixed;
    valid.fixed_cwd = directory
        .join("work/../work")
        .to_string_lossy()
        .into_owned();
    valid.exit_behavior = ExitBehavior::CloseOnSuccess;
    assert!(validate_profile_with(&valid, |_| unreachable!()).is_ok());

    let mut invalid = Vec::new();
    let mut value = valid.clone();
    value.id.clear();
    invalid.push(value);
    let mut value = valid.clone();
    value.id = "bad id".to_owned();
    invalid.push(value);
    let mut value = valid.clone();
    value.name = "\n".to_owned();
    invalid.push(value);
    let mut value = valid.clone();
    value.provider = "x".repeat(129);
    invalid.push(value);
    let mut value = valid.clone();
    value.args = vec![String::new(); 65];
    invalid.push(value);
    let mut value = valid.clone();
    value.font_size = MAX_PROFILE_FONT_SIZE + 1;
    invalid.push(value);
    let mut value = valid.clone();
    value.cwd_policy = CwdPolicy::Project;
    invalid.push(value);
    let mut value = valid.clone();
    value
        .env
        .insert("PTRACK_CAPABILITY_TOKEN".to_owned(), "x".to_owned());
    invalid.push(value);
    let mut value = valid.clone();
    value
        .env
        .insert("OPENAI_API_KEY".to_owned(), "x".to_owned());
    invalid.push(value);
    let mut value = valid;
    value.args.push("bad\0arg".to_owned());
    invalid.push(value);

    for value in invalid {
        assert!(
            validate_profile_with(&value, |_| unreachable!()).is_err(),
            "accepted invalid profile: {value:?}"
        );
    }
    remove_test_directory(&directory);
}

#[test]
fn relative_executable_is_resolved_once_to_an_absolute_path() {
    let directory = test_directory("profile-lookup");
    let mut input = profile("shell", ProfileKind::Shell, Path::new("shell-under-test"));
    let expected = directory.join("shell-under-test");
    let validated = validate_profile_with(&input, |name| {
        assert_eq!(name, "shell-under-test");
        Ok(expected.clone())
    })
    .unwrap();
    assert_eq!(validated.executable, expected.to_string_lossy());
    assert_eq!(input.executable, "shell-under-test");

    input.executable = "missing".to_owned();
    assert!(
        validate_profile_with(&input, |_| Err(io::Error::from(io::ErrorKind::NotFound))).is_err()
    );
    remove_test_directory(&directory);
}

#[cfg(unix)]
#[test]
fn executable_availability_requires_an_executable_file() {
    use std::os::unix::fs::PermissionsExt;

    let directory = test_directory("profile-availability");
    let executable = directory.join("agent");
    fs::write(&executable, b"#!/bin/sh\n").unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let input = profile("agent-test", ProfileKind::Agent, &executable);
    assert!(profile_executable_is_available(&input));

    fs::set_permissions(&executable, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(!profile_executable_is_available(&input));
    assert!(!profile_executable_is_available(&profile(
        "agent-missing",
        ProfileKind::Agent,
        &directory.join("missing"),
    )));
    remove_test_directory(&directory);
}

#[test]
fn profiles_sort_default_shell_then_shells_then_agents() {
    let executable = Path::new("/bin/example");
    let mut profiles = vec![
        profile("agent-z", ProfileKind::Agent, executable),
        profile("shell-z", ProfileKind::Shell, executable),
        profile("agent-a", ProfileKind::Agent, executable),
        profile("shell-default", ProfileKind::Shell, executable),
        profile("shell-a", ProfileKind::Shell, executable),
    ];
    sort_profiles(&mut profiles);
    assert_eq!(
        profiles
            .iter()
            .map(|profile| profile.id.as_str())
            .collect::<Vec<_>>(),
        ["shell-default", "shell-a", "shell-z", "agent-a", "agent-z"]
    );
}

#[test]
fn discovery_uses_authoritative_shell_and_fixed_agent_order() {
    let directory = test_directory("profile-discovery");
    let account_shell = directory.join("zsh");
    let inherited_shell = directory.join("bash");
    let installed = HashMap::from([
        ("agy".to_owned(), directory.join("agy")),
        ("codex".to_owned(), directory.join("codex")),
        ("opencode".to_owned(), directory.join("opencode")),
    ]);
    let lookup = |name: &str| {
        installed
            .get(name)
            .cloned()
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
    };
    let user_shell = || Ok(account_shell.clone());
    let profiles = discover_profiles_with(
        "darwin",
        lookup,
        |name| match name {
            "SHELL" => inherited_shell.to_string_lossy().into_owned(),
            _ => String::new(),
        },
        Some(&user_shell),
    )
    .unwrap();
    assert_eq!(
        profiles
            .iter()
            .map(|profile| profile.id.as_str())
            .collect::<Vec<_>>(),
        [
            "shell-default",
            "agent-agy",
            "agent-codex",
            "agent-opencode"
        ]
    );
    assert_eq!(profiles[0].executable, account_shell.to_string_lossy());
    assert_eq!(profiles[0].args, ["-l"]);
    remove_test_directory(&directory);
}

#[cfg(unix)]
#[test]
fn discovery_uses_darwin_agent_fallbacks() {
    let directory = test_directory("profile-discovery-fallback");
    let shell = directory.join("zsh");
    let gemini = PathBuf::from("/opt/homebrew/bin/gemini");
    let lookup = |name: &str| {
        if name == gemini.to_string_lossy() {
            Ok(gemini.clone())
        } else {
            Err(io::Error::from(io::ErrorKind::NotFound))
        }
    };
    let no_user_shell = || Err(io::Error::from(io::ErrorKind::NotFound));
    let profiles = discover_profiles_with(
        "darwin",
        lookup,
        |name| match name {
            "SHELL" => shell.to_string_lossy().into_owned(),
            _ => String::new(),
        },
        Some(&no_user_shell),
    )
    .unwrap();
    assert!(profiles.iter().any(|profile| profile.id == "agent-gemini"));
    remove_test_directory(&directory);
}

#[test]
fn discovery_covers_path_local_bin_homebrew_and_kimi_home_and_filters_absent_agents() {
    let directory = test_directory("profile-discovery-install-locations");
    let shell = directory.join("zsh");
    let home = directory.join("home");
    let installed = HashMap::from([
        ("agy".to_owned(), directory.join("path/agy")),
        (
            home.join(".local/bin/claude")
                .to_string_lossy()
                .into_owned(),
            home.join(".local/bin/claude"),
        ),
        (
            "/opt/homebrew/bin/codex".to_owned(),
            PathBuf::from("/opt/homebrew/bin/codex"),
        ),
        (
            home.join(".local/bin/cursor-agent")
                .to_string_lossy()
                .into_owned(),
            home.join(".local/bin/cursor-agent"),
        ),
        (
            home.join(".kimi-code/bin/kimi")
                .to_string_lossy()
                .into_owned(),
            home.join(".kimi-code/bin/kimi"),
        ),
    ]);
    let lookup = |name: &str| {
        installed
            .get(name)
            .cloned()
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
    };
    let user_shell = || Ok(shell.clone());
    let profiles = discover_profiles_with(
        "macos",
        lookup,
        |name| match name {
            "HOME" => home.to_string_lossy().into_owned(),
            _ => String::new(),
        },
        Some(&user_shell),
    )
    .unwrap();

    assert_eq!(
        profiles
            .iter()
            .map(|profile| profile.id.as_str())
            .collect::<Vec<_>>(),
        [
            "shell-default",
            "agent-agy",
            "agent-claude",
            "agent-codex",
            "agent-cursor",
            "agent-kimi",
        ]
    );
    assert!(
        !profiles
            .iter()
            .any(|profile| profile.id == "agent-opencode")
    );
    remove_test_directory(&directory);
}

#[test]
fn discovery_uses_windows_comspec() {
    let directory = test_directory("profile-discovery-comspec");
    let comspec = directory.join("cmd.exe");
    let profiles = discover_profiles_with(
        "windows",
        |_| Err(io::Error::from(io::ErrorKind::NotFound)),
        |name| match name {
            "COMSPEC" => comspec.to_string_lossy().into_owned(),
            _ => String::new(),
        },
        None::<&fn() -> io::Result<PathBuf>>,
    )
    .unwrap();
    assert_eq!(profiles.len(), 1);
    assert_eq!(profiles[0].executable, comspec.to_string_lossy());
    assert!(profiles[0].args.is_empty());
    remove_test_directory(&directory);
}

#[test]
fn environment_filters_authority_and_applies_deterministic_terminal_defaults() {
    let base = vec![
        "PATH=/usr/bin".to_owned(),
        "NO_COLOR=1".to_owned(),
        "TERM=vt100".to_owned(),
        "PTRACK_HOME=/tmp/home".to_owned(),
        "PTRACK_CAPABILITY_TOKEN=stale".to_owned(),
    ];
    let overrides = BTreeMap::from([
        ("TERM".to_owned(), "screen-256color".to_owned()),
        ("PTRACK_CAPABILITY_TOKEN".to_owned(), "fresh".to_owned()),
    ]);
    let environment = build_environment_for_os(&base, &overrides, "linux").unwrap();
    assert_eq!(
        environment,
        [
            "COLORTERM=truecolor",
            "LANG=C.UTF-8",
            "PATH=/usr/bin",
            "PTRACK_CAPABILITY_TOKEN=fresh",
            "PTRACK_HOME=/tmp/home",
            "TERM=screen-256color",
            "TERM_PROGRAM=p-track",
        ]
    );
}

#[test]
fn windows_environment_normalizes_keys_and_drops_drive_entries() {
    let base = vec![
        r"=C:=C:\work".to_owned(),
        "=ExitCode=00000000".to_owned(),
        r"Path=C:\Windows".to_owned(),
        "term=vt100".to_owned(),
        "PAIR=a=b".to_owned(),
    ];
    let overrides = BTreeMap::from([
        ("PATH".to_owned(), r"C:\Tools".to_owned()),
        ("TERM".to_owned(), "screen-256color".to_owned()),
    ]);
    let environment = build_environment_for_os(&base, &overrides, "windows").unwrap();
    assert_eq!(
        environment,
        [
            "COLORTERM=truecolor",
            "PAIR=a=b",
            r"PATH=C:\Tools",
            "TERM=screen-256color",
            "TERM_PROGRAM=p-track",
        ]
    );
}

#[test]
fn working_directory_resolution_defaults_and_rejects_non_directories() {
    let directory = test_directory("profile-cwd");
    let child = directory.join("child");
    fs::create_dir(&child).unwrap();
    let file = directory.join("file");
    fs::write(&file, b"file").unwrap();
    assert_eq!(resolve_cwd(&directory, None).unwrap(), directory);
    assert_eq!(resolve_cwd(&directory, Some(&child)).unwrap(), child);
    assert!(resolve_cwd(&directory, Some(&file)).is_err());
    assert!(resolve_cwd(&directory, Some(&directory.join("missing"))).is_err());
    remove_test_directory(&directory);
}

static TEST_DIRECTORY_COUNTER: AtomicU64 = AtomicU64::new(0);

fn test_directory(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "ptrack-terminal-{label}-{}-{}",
        std::process::id(),
        TEST_DIRECTORY_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

fn remove_test_directory(path: &Path) {
    let _ = fs::remove_dir_all(path);
}
