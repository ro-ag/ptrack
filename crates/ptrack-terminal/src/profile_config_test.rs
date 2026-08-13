use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::profile::{
    CwdPolicy, DEFAULT_PROFILE_THEME, ExitBehavior, MAX_PROFILE_FONT_SIZE, Profile, ProfileKind,
};
use crate::profile_config::{
    MAX_CONFIGURED_PROFILES, PROFILE_CONFIG_VERSION, ProfileConfig, load_profile_config,
    load_profile_config_if_exists, merge_profiles, profile_config_path, save_profile_config,
    validate_profile_config,
};

fn profile(id: &str, kind: ProfileKind, executable: &Path) -> Profile {
    Profile {
        id: id.to_owned(),
        name: id.to_owned(),
        kind,
        provider: if kind == ProfileKind::Agent {
            id.strip_prefix("agent-").unwrap_or("test").to_owned()
        } else {
            String::new()
        },
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
fn global_profile_config_path_is_not_project_scoped() {
    let home = Path::new("/global/ptrack-home");
    assert_eq!(
        profile_config_path(home),
        home.join("terminal-profiles.json")
    );
}

#[test]
fn config_validation_enforces_version_count_and_unique_profiles() {
    let directory = test_directory("config-validation");
    let executable = directory.join("shell");
    let valid = ProfileConfig {
        version: PROFILE_CONFIG_VERSION,
        profiles: vec![profile("shell", ProfileKind::Shell, &executable)],
    };
    let normalized = validate_profile_config(&valid).unwrap();
    assert_eq!(normalized.profiles[0].theme, DEFAULT_PROFILE_THEME);

    let mut wrong_version = valid.clone();
    wrong_version.version = 2;
    assert!(validate_profile_config(&wrong_version).is_err());
    let duplicate = ProfileConfig {
        version: PROFILE_CONFIG_VERSION,
        profiles: vec![valid.profiles[0].clone(), valid.profiles[0].clone()],
    };
    assert!(validate_profile_config(&duplicate).is_err());
    let too_many = ProfileConfig {
        version: PROFILE_CONFIG_VERSION,
        profiles: (0..=MAX_CONFIGURED_PROFILES)
            .map(|index| profile(&format!("shell-{index}"), ProfileKind::Shell, &executable))
            .collect(),
    };
    assert!(validate_profile_config(&too_many).is_err());
    remove_test_directory(&directory);
}

#[test]
fn strict_load_rejects_unknown_fields_trailing_data_empty_and_oversize() {
    let directory = test_directory("config-strict-load");
    let cases = [
        ("version", r#"{"version":2,"profiles":[]}"#.to_owned()),
        (
            "unknown-root",
            r#"{"version":1,"profiles":[],"extra":true}"#.to_owned(),
        ),
        (
            "unknown-profile",
            r#"{"version":1,"profiles":[{"id":"shell","name":"Shell","kind":"shell","executable":"/bin/sh","extra":true}]}"#.to_owned(),
        ),
        (
            "trailing",
            r#"{"version":1,"profiles":[]} {}"#.to_owned(),
        ),
        ("empty", String::new()),
        ("oversize", " ".repeat(256 * 1_024 + 1)),
    ];
    for (name, contents) in cases {
        let path = directory.join(format!("{name}.json"));
        fs::write(&path, contents).unwrap();
        assert!(
            load_profile_config(&path).is_err(),
            "accepted invalid case {name}"
        );
    }
    remove_test_directory(&directory);
}

#[test]
fn missing_config_is_the_only_nonfatal_load_result() {
    let directory = test_directory("config-missing");
    let missing = directory.join("missing.json");
    assert_eq!(load_profile_config_if_exists(&missing).unwrap(), None);

    let invalid = directory.join("invalid.json");
    fs::write(&invalid, b"{}").unwrap();
    assert!(load_profile_config_if_exists(&invalid).is_err());
    remove_test_directory(&directory);
}

#[test]
fn private_atomic_round_trip_replaces_content_without_environment_snapshot() {
    let root = test_directory("config-round-trip");
    let private_directory = root.join("new-private-home");
    let path = private_directory.join("terminal-profiles.json");
    let executable = root.join("shell");
    let mut first = profile("shell-default", ProfileKind::Shell, &executable);
    first.name = "First".to_owned();
    first.env.insert("PAGER".to_owned(), "less".to_owned());
    save_profile_config(
        &path,
        &ProfileConfig {
            version: PROFILE_CONFIG_VERSION,
            profiles: vec![first.clone()],
        },
    )
    .unwrap();

    first.name = "Second".to_owned();
    first.env.insert("PAGER".to_owned(), "more".to_owned());
    save_profile_config(
        &path,
        &ProfileConfig {
            version: PROFILE_CONFIG_VERSION,
            profiles: vec![first],
        },
    )
    .unwrap();

    let contents = fs::read_to_string(&path).unwrap();
    assert!(contents.ends_with('\n'));
    assert!(!contents.contains("PTRACK_CAPABILITY_TOKEN"));
    assert_eq!(
        fs::read_dir(&private_directory).unwrap().count(),
        1,
        "atomic save left a temporary file"
    );
    let loaded = load_profile_config(&path).unwrap();
    assert_eq!(loaded.profiles[0].name, "Second");
    assert_eq!(loaded.profiles[0].env["PAGER"], "more");
    assert_eq!(loaded.profiles[0].theme, DEFAULT_PROFILE_THEME);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&private_directory)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
    remove_test_directory(&root);
}

#[test]
fn merge_replaces_stable_ids_adds_shells_and_preserves_agent_launch_identity() {
    let directory = test_directory("config-merge");
    let shell_executable = directory.join("shell");
    let agent_executable = directory.join("codex");
    let mut shell = profile("shell-default", ProfileKind::Shell, &shell_executable);
    shell.name = "Default shell".to_owned();
    let mut agent = profile("agent-codex", ProfileKind::Agent, &agent_executable);
    agent.args = vec!["--discovered".to_owned()];
    agent.env.insert("CODEX_MODE".to_owned(), "base".to_owned());

    let mut presentation_override = agent.clone();
    presentation_override.name = "Codex focused".to_owned();
    presentation_override.theme = "solarized-dark".to_owned();
    presentation_override.font_size = MAX_PROFILE_FONT_SIZE;
    presentation_override.exit_behavior = ExitBehavior::CloseOnSuccess;
    let custom = profile("shell-tools", ProfileKind::Shell, &shell_executable);
    let merged = merge_profiles(&[agent.clone(), shell], &[custom, presentation_override]).unwrap();
    assert_eq!(
        merged
            .iter()
            .map(|profile| profile.id.as_str())
            .collect::<Vec<_>>(),
        ["shell-default", "shell-tools", "agent-codex"]
    );
    assert_eq!(merged[2].args, agent.args);
    assert_eq!(merged[2].env, agent.env);
    assert_eq!(merged[2].theme, "solarized-dark");

    let mut changed = agent.clone();
    changed.args.push("--changed".to_owned());
    assert!(merge_profiles(&[agent], &[changed]).is_err());
    let custom_agent = profile("agent-custom", ProfileKind::Agent, &agent_executable);
    assert!(merge_profiles(&[], &[custom_agent]).is_err());
    remove_test_directory(&directory);
}

#[test]
fn merge_rejects_kind_provider_and_duplicate_repurposing() {
    let directory = test_directory("config-merge-reject");
    let executable = directory.join("codex");
    let agent = profile("agent-codex", ProfileKind::Agent, &executable);

    assert!(merge_profiles(&[], &[agent.clone(), agent.clone()]).is_err());
    let mut changed_kind = agent.clone();
    changed_kind.kind = ProfileKind::Shell;
    changed_kind.provider.clear();
    assert!(merge_profiles(std::slice::from_ref(&agent), &[changed_kind]).is_err());
    let mut changed_provider = agent.clone();
    changed_provider.provider = "other".to_owned();
    assert!(merge_profiles(std::slice::from_ref(&agent), &[changed_provider]).is_err());
    let mut changed_cwd = agent.clone();
    changed_cwd.cwd_policy = CwdPolicy::Project;
    assert!(merge_profiles(&[agent], &[changed_cwd]).is_err());
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
