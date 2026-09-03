use std::collections::BTreeMap;
use std::env;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Deserializer, Serialize};

pub const DEFAULT_PROFILE_THEME: &str = "default";
pub const DEFAULT_PROFILE_FONT_FAMILY: &str = "monospace";
pub const DEFAULT_PROFILE_FONT_SIZE: u16 = 14;
pub const DEFAULT_PROFILE_SCROLLBACK: u32 = 25_000;
pub const MIN_PROFILE_FONT_SIZE: u16 = 10;
pub const MAX_PROFILE_FONT_SIZE: u16 = 24;
pub const MIN_PROFILE_SCROLLBACK: u32 = 100;
pub const MAX_PROFILE_SCROLLBACK: u32 = 100_000;

const MAX_PROFILE_ID_BYTES: usize = 128;
const MAX_PROFILE_NAME_BYTES: usize = 256;
const MAX_PROFILE_PROVIDER_BYTES: usize = 128;
const MAX_PROFILE_EXECUTABLE_BYTES: usize = 4_096;
const MAX_PROFILE_ARGUMENT_COUNT: usize = 64;
const MAX_PROFILE_ARGUMENT_BYTES: usize = 4_096;
const MAX_PROFILE_ARGUMENTS_BYTES: usize = 64 * 1_024;
const MAX_PROFILE_ENVIRONMENT_COUNT: usize = 64;
const MAX_PROFILE_ENVIRONMENT_KEY_BYTES: usize = 128;
const MAX_PROFILE_ENVIRONMENT_VALUE_BYTES: usize = 4_096;
const MAX_PROFILE_ENVIRONMENT_BYTES: usize = 64 * 1_024;
const MAX_PROFILE_THEME_BYTES: usize = 64;
const MAX_PROFILE_FONT_FAMILY_BYTES: usize = 256;
const MAX_PROFILE_CWD_BYTES: usize = 4_096;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProfileKind {
    Shell,
    Agent,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CwdPolicy {
    #[default]
    Requested,
    Project,
    Fixed,
}

impl<'de> Deserialize<'de> for CwdPolicy {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        match value.as_str() {
            "" | "requested" => Ok(Self::Requested),
            "project" => Ok(Self::Project),
            "fixed" => Ok(Self::Fixed),
            _ => Err(serde::de::Error::unknown_variant(
                &value,
                &["requested", "project", "fixed"],
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExitBehavior {
    #[default]
    Keep,
    CloseOnSuccess,
    Close,
}

impl<'de> Deserialize<'de> for ExitBehavior {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        match value.as_str() {
            "" | "keep" => Ok(Self::Keep),
            "close-on-success" => Ok(Self::CloseOnSuccess),
            "close" => Ok(Self::Close),
            _ => Err(serde::de::Error::unknown_variant(
                &value,
                &["keep", "close-on-success", "close"],
            )),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub kind: ProfileKind,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provider: String,
    pub executable: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub theme: String,
    #[serde(default, rename = "fontFamily")]
    pub font_family: String,
    #[serde(default, rename = "fontSize")]
    pub font_size: u16,
    #[serde(default)]
    pub scrollback: u32,
    #[serde(default, rename = "cwdPolicy")]
    pub cwd_policy: CwdPolicy,
    #[serde(default, rename = "fixedCwd", skip_serializing_if = "String::is_empty")]
    pub fixed_cwd: String,
    #[serde(default, rename = "exitBehavior")]
    pub exit_behavior: ExitBehavior,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileError(String);

impl ProfileError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ProfileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ProfileError {}

/// Validates and normalizes one independently owned terminal profile.
///
/// # Errors
///
/// Returns an error when any identity, launch, presentation, policy, or size
/// invariant is violated, or when a relative executable cannot be resolved.
pub fn validate_profile(profile: &Profile) -> Result<Profile, ProfileError> {
    validate_profile_with(profile, look_path)
}

pub(crate) fn validate_profile_with<F>(
    profile: &Profile,
    look_path_fn: F,
) -> Result<Profile, ProfileError>
where
    F: Fn(&str) -> io::Result<PathBuf>,
{
    if !is_stable_name(&profile.id) {
        return Err(ProfileError::new("profile ID must be stable and nonempty"));
    }
    if profile.id.len() > MAX_PROFILE_ID_BYTES {
        return Err(ProfileError::new("profile ID is too long"));
    }
    if profile.name.trim().is_empty() {
        return Err(ProfileError::new("profile name must be nonempty"));
    }
    if profile.name.len() > MAX_PROFILE_NAME_BYTES {
        return Err(ProfileError::new("profile name is too long"));
    }

    let mut normalized = profile.clone();
    if normalized.kind == ProfileKind::Agent && normalized.provider.trim().is_empty() {
        normalized.provider = normalized
            .id
            .strip_prefix("agent-")
            .unwrap_or(&normalized.id)
            .to_owned();
    }
    if normalized.kind == ProfileKind::Agent && normalized.provider.trim().is_empty() {
        return Err(ProfileError::new("agent profile provider must be nonempty"));
    }
    if normalized.provider.len() > MAX_PROFILE_PROVIDER_BYTES {
        return Err(ProfileError::new("profile provider is too long"));
    }
    if normalized.executable.trim().is_empty() {
        return Err(ProfileError::new("profile executable must be nonempty"));
    }
    if normalized.executable.len() > MAX_PROFILE_EXECUTABLE_BYTES {
        return Err(ProfileError::new("profile executable is too long"));
    }
    if [
        &normalized.id,
        &normalized.name,
        &normalized.provider,
        &normalized.executable,
    ]
    .into_iter()
    .any(|value| contains_nul(value))
    {
        return Err(ProfileError::new("profile contains a NUL value"));
    }

    if normalized.args.len() > MAX_PROFILE_ARGUMENT_COUNT {
        return Err(ProfileError::new("profile has too many arguments"));
    }
    let mut argument_bytes = 0_usize;
    for argument in &normalized.args {
        if contains_nul(argument) {
            return Err(ProfileError::new("profile argument contains NUL"));
        }
        if argument.len() > MAX_PROFILE_ARGUMENT_BYTES {
            return Err(ProfileError::new("profile argument is too long"));
        }
        argument_bytes = argument_bytes.saturating_add(argument.len());
    }
    if argument_bytes > MAX_PROFILE_ARGUMENTS_BYTES {
        return Err(ProfileError::new("profile arguments are too large"));
    }

    if normalized.env.len() > MAX_PROFILE_ENVIRONMENT_COUNT {
        return Err(ProfileError::new(
            "profile has too many environment overrides",
        ));
    }
    let mut environment_bytes = 0_usize;
    for (key, value) in &normalized.env {
        if !safe_profile_environment_entry(key, value) {
            return Err(ProfileError::new(format!(
                "profile environment override {key:?} is unsafe"
            )));
        }
        environment_bytes = environment_bytes.saturating_add(key.len() + value.len());
    }
    if environment_bytes > MAX_PROFILE_ENVIRONMENT_BYTES {
        return Err(ProfileError::new(
            "profile environment overrides are too large",
        ));
    }

    normalize_presentation(&mut normalized)?;
    if !Path::new(&normalized.executable).is_absolute() {
        let mut resolved = look_path_fn(&normalized.executable).map_err(|error| {
            ProfileError::new(format!(
                "resolve profile executable {:?}: {error}",
                normalized.executable
            ))
        })?;
        if !resolved.is_absolute() {
            resolved = env::current_dir()
                .map_err(|error| {
                    ProfileError::new(format!("make profile executable absolute: {error}"))
                })?
                .join(resolved);
        }
        normalized.executable = normalize_path(&resolved).to_string_lossy().into_owned();
    }
    Ok(normalized)
}

fn normalize_presentation(profile: &mut Profile) -> Result<(), ProfileError> {
    if profile.theme.is_empty() {
        DEFAULT_PROFILE_THEME.clone_into(&mut profile.theme);
    }
    if profile.theme.len() > MAX_PROFILE_THEME_BYTES || !is_stable_name(&profile.theme) {
        return Err(ProfileError::new(
            "profile theme must be a bounded stable name",
        ));
    }
    if profile.font_family.is_empty() {
        DEFAULT_PROFILE_FONT_FAMILY.clone_into(&mut profile.font_family);
    }
    if profile.font_family.len() > MAX_PROFILE_FONT_FAMILY_BYTES
        || contains_nul(&profile.font_family)
        || profile.font_family.trim().is_empty()
    {
        return Err(ProfileError::new("profile font family is invalid"));
    }
    if profile.font_size == 0 {
        profile.font_size = DEFAULT_PROFILE_FONT_SIZE;
    }
    if !(MIN_PROFILE_FONT_SIZE..=MAX_PROFILE_FONT_SIZE).contains(&profile.font_size) {
        return Err(ProfileError::new(format!(
            "profile font size must be between {MIN_PROFILE_FONT_SIZE} and {MAX_PROFILE_FONT_SIZE}"
        )));
    }
    if profile.scrollback == 0 {
        profile.scrollback = DEFAULT_PROFILE_SCROLLBACK;
    }
    if !(MIN_PROFILE_SCROLLBACK..=MAX_PROFILE_SCROLLBACK).contains(&profile.scrollback) {
        return Err(ProfileError::new(format!(
            "profile scrollback must be between {MIN_PROFILE_SCROLLBACK} and {MAX_PROFILE_SCROLLBACK}"
        )));
    }
    match profile.cwd_policy {
        CwdPolicy::Requested | CwdPolicy::Project => {
            if !profile.fixed_cwd.is_empty() {
                return Err(ProfileError::new(
                    "profile fixed working directory requires fixed policy",
                ));
            }
        }
        CwdPolicy::Fixed => {
            let fixed = Path::new(&profile.fixed_cwd);
            if profile.fixed_cwd.is_empty()
                || profile.fixed_cwd.len() > MAX_PROFILE_CWD_BYTES
                || contains_nul(&profile.fixed_cwd)
                || !fixed.is_absolute()
            {
                return Err(ProfileError::new(
                    "profile fixed working directory must be a bounded absolute path",
                ));
            }
            profile.fixed_cwd = normalize_path(fixed).to_string_lossy().into_owned();
        }
    }
    Ok(())
}

pub fn sort_profiles(profiles: &mut [Profile]) {
    profiles.sort_by(|left, right| {
        profile_rank(left)
            .cmp(&profile_rank(right))
            .then_with(|| left.id.cmp(&right.id))
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.provider.cmp(&right.provider))
    });
}

fn profile_rank(profile: &Profile) -> u8 {
    match (profile.kind, profile.id.as_str()) {
        (ProfileKind::Shell, "shell-default") => 0,
        (ProfileKind::Shell, _) => 1,
        (ProfileKind::Agent, _) => 2,
    }
}

struct AgentCandidate {
    id: &'static str,
    name: &'static str,
    provider: &'static str,
    executable: &'static str,
}

const SUPPORTED_AGENT_CANDIDATES: [AgentCandidate; 7] = [
    AgentCandidate {
        id: "agent-agy",
        name: "Agy",
        provider: "agy",
        executable: "agy",
    },
    AgentCandidate {
        id: "agent-claude",
        name: "Claude Code",
        provider: "claude",
        executable: "claude",
    },
    AgentCandidate {
        id: "agent-codex",
        name: "Codex",
        provider: "codex",
        executable: "codex",
    },
    AgentCandidate {
        id: "agent-cursor",
        name: "Cursor Agent",
        provider: "cursor",
        executable: "cursor-agent",
    },
    AgentCandidate {
        id: "agent-gemini",
        name: "Gemini",
        provider: "gemini",
        executable: "gemini",
    },
    AgentCandidate {
        id: "agent-kimi",
        name: "Kimi Code",
        provider: "kimi",
        executable: "kimi",
    },
    AgentCandidate {
        id: "agent-opencode",
        name: "OpenCode",
        provider: "opencode",
        executable: "opencode",
    },
];

/// Discovers the login shell and installed supported agent CLIs.
///
/// # Errors
///
/// Returns an error when no default shell can be resolved or the resulting
/// default shell profile fails validation.
pub fn discover_profiles() -> Result<Vec<Profile>, ProfileError> {
    let user_shell = || directory_services_user_shell();
    discover_profiles_with(
        env::consts::OS,
        look_path,
        |name| env::var(name).unwrap_or_default(),
        Some(&user_shell),
    )
}

/// Reports whether a normalized profile still names an executable file.
#[must_use]
pub fn profile_executable_is_available(profile: &Profile) -> bool {
    look_path(&profile.executable).is_ok()
}

pub(crate) fn discover_profiles_with<L, G, U>(
    os: &str,
    look_path_fn: L,
    getenv: G,
    user_shell: Option<&U>,
) -> Result<Vec<Profile>, ProfileError>
where
    L: Fn(&str) -> io::Result<PathBuf> + Copy,
    G: Fn(&str) -> String,
    U: Fn() -> io::Result<PathBuf>,
{
    let (shell_executable, shell_args) =
        discover_default_shell(os, look_path_fn, &getenv, user_shell)?;
    let shell = validate_profile_with(
        &Profile {
            id: "shell-default".to_owned(),
            name: "Default shell".to_owned(),
            kind: ProfileKind::Shell,
            provider: String::new(),
            executable: shell_executable.to_string_lossy().into_owned(),
            args: shell_args,
            env: BTreeMap::new(),
            theme: String::new(),
            font_family: String::new(),
            font_size: 0,
            scrollback: 0,
            cwd_policy: CwdPolicy::default(),
            fixed_cwd: String::new(),
            exit_behavior: ExitBehavior::default(),
        },
        look_path_fn,
    )
    .map_err(|error| ProfileError::new(format!("validate default shell: {error}")))?;

    let mut profiles = vec![shell];
    for candidate in SUPPORTED_AGENT_CANDIDATES {
        let Ok(executable) =
            discover_agent_executable(candidate.executable, os, look_path_fn, &getenv)
        else {
            continue;
        };
        let profile = validate_profile_with(
            &Profile {
                id: candidate.id.to_owned(),
                name: candidate.name.to_owned(),
                kind: ProfileKind::Agent,
                provider: candidate.provider.to_owned(),
                executable: executable.to_string_lossy().into_owned(),
                args: Vec::new(),
                env: BTreeMap::new(),
                theme: String::new(),
                font_family: String::new(),
                font_size: 0,
                scrollback: 0,
                cwd_policy: CwdPolicy::default(),
                fixed_cwd: String::new(),
                exit_behavior: ExitBehavior::default(),
            },
            look_path_fn,
        )
        .map_err(|error| {
            ProfileError::new(format!(
                "validate discovered {} profile: {error}",
                candidate.name
            ))
        })?;
        profiles.push(profile);
    }
    Ok(profiles)
}

fn discover_default_shell<L, G, U>(
    os: &str,
    look_path_fn: L,
    getenv: &G,
    user_shell: Option<&U>,
) -> Result<(PathBuf, Vec<String>), ProfileError>
where
    L: Fn(&str) -> io::Result<PathBuf> + Copy,
    G: Fn(&str) -> String,
    U: Fn() -> io::Result<PathBuf>,
{
    if os == "windows" {
        let comspec = getenv("COMSPEC");
        let executable = if comspec.is_empty() {
            look_path_fn("cmd.exe")
                .map_err(|_| ProfileError::new("default Windows command processor not found"))?
        } else {
            PathBuf::from(comspec)
        };
        return Ok((executable, Vec::new()));
    }

    let mut executable = if os == "macos" || os == "darwin" {
        user_shell.and_then(|resolve| resolve().ok())
    } else {
        None
    };
    if executable.is_none() {
        let inherited = getenv("SHELL");
        if !inherited.is_empty() {
            executable = Some(PathBuf::from(inherited));
        }
    }
    if executable.is_none() && (os == "macos" || os == "darwin") {
        executable = look_path_fn("zsh").ok();
    }
    if executable.is_none() {
        executable =
            Some(look_path_fn("sh").map_err(|_| ProfileError::new("default shell not found"))?);
    }
    Ok((
        executable.expect("default shell established"),
        vec!["-l".to_owned()],
    ))
}

fn discover_agent_executable<L, G>(
    name: &str,
    os: &str,
    look_path_fn: L,
    getenv: &G,
) -> io::Result<PathBuf>
where
    L: Fn(&str) -> io::Result<PathBuf>,
    G: Fn(&str) -> String,
{
    let original_error = match look_path_fn(name) {
        Ok(path) => return Ok(path),
        Err(error) => error,
    };
    if os != "macos" && os != "darwin" {
        return Err(original_error);
    }

    let mut candidates = vec![
        PathBuf::from("/opt/homebrew/bin").join(name),
        PathBuf::from("/usr/local/bin").join(name),
    ];
    let home = getenv("HOME");
    if !home.is_empty() {
        candidates.push(Path::new(&home).join(".local/bin").join(name));
        candidates.push(Path::new(&home).join(".kimi-code/bin").join(name));
        candidates.push(Path::new(&home).join(".opencode/bin").join(name));
    }
    for candidate in candidates {
        if let Ok(path) = look_path_fn(&candidate.to_string_lossy()) {
            return Ok(path);
        }
    }
    Err(original_error)
}

/// Builds a deterministic child environment from inherited and launch values.
///
/// # Errors
///
/// Returns an error when an inherited entry is malformed or an override has an
/// invalid key or contains a NUL byte.
pub fn build_environment(
    base: &[String],
    overrides: &BTreeMap<String, String>,
) -> Result<Vec<String>, ProfileError> {
    build_environment_for_os(base, overrides, env::consts::OS)
}

pub(crate) fn build_environment_for_os(
    base: &[String],
    overrides: &BTreeMap<String, String>,
    os: &str,
) -> Result<Vec<String>, ProfileError> {
    let normalize = |key: &str| {
        if os == "windows" {
            key.to_uppercase()
        } else {
            key.to_owned()
        }
    };
    let mut values = BTreeMap::<String, (String, String)>::new();
    for entry in base {
        // cmd.exe exports per-drive working-directory bookkeeping under names
        // that start with '=' (`=C:`, `=ExitCode`). Windows rejects any
        // environment name containing '=', so no child can ever receive them.
        if entry.starts_with('=') {
            continue;
        }
        let Some((key, value)) = entry.split_once('=') else {
            return Err(ProfileError::new(format!(
                "invalid inherited environment entry {entry:?}"
            )));
        };
        if contains_nul(key) || contains_nul(value) {
            return Err(ProfileError::new(format!(
                "invalid inherited environment entry {key:?}"
            )));
        }
        let upper = key.to_uppercase();
        if upper.starts_with("PTRACK_") && upper != "PTRACK_HOME" {
            continue;
        }
        values.insert(normalize(key), (key.to_owned(), value.to_owned()));
    }
    values.remove(&normalize("NO_COLOR"));
    for (key, value) in [
        ("TERM", "xterm-256color"),
        ("COLORTERM", "truecolor"),
        ("TERM_PROGRAM", "p-track"),
    ] {
        values.insert(normalize(key), (key.to_owned(), value.to_owned()));
    }
    for (key, value) in overrides {
        if !safe_environment_entry(key, value) {
            return Err(ProfileError::new(format!(
                "unsafe environment override {key:?}"
            )));
        }
        values.insert(normalize(key), (key.clone(), value.clone()));
    }

    let has_locale = ["LC_ALL", "LC_CTYPE", "LANG"].iter().any(|key| {
        values
            .get(&normalize(key))
            .is_some_and(|(_, value)| !value.is_empty())
    });
    if !has_locale {
        let locale = match os {
            "macos" | "darwin" => Some("en_US.UTF-8"),
            "windows" => None,
            _ => Some("C.UTF-8"),
        };
        if let Some(locale) = locale {
            values.insert(normalize("LANG"), ("LANG".to_owned(), locale.to_owned()));
        }
    }

    Ok(values
        .into_values()
        .map(|(key, value)| format!("{key}={value}"))
        .collect())
}

/// Resolves and validates the requested terminal working directory.
///
/// # Errors
///
/// Returns an error when the selected path cannot be made absolute, does not
/// exist, or is not a directory.
pub fn resolve_cwd(project_root: &Path, requested: Option<&Path>) -> Result<PathBuf, ProfileError> {
    let selected = requested.unwrap_or(project_root);
    let absolute = if selected.is_absolute() {
        selected.to_owned()
    } else {
        env::current_dir()
            .map_err(|error| ProfileError::new(format!("resolve working directory: {error}")))?
            .join(selected)
    };
    let metadata = fs::metadata(&absolute)
        .map_err(|error| ProfileError::new(format!("stat working directory: {error}")))?;
    if !metadata.is_dir() {
        return Err(ProfileError::new(format!(
            "working directory {} is not a directory",
            absolute.display()
        )));
    }
    Ok(normalize_path(&absolute))
}

pub(crate) fn safe_environment_entry(key: &str, value: &str) -> bool {
    !key.is_empty() && !key.contains(['=', '\0']) && !contains_nul(value)
}

fn safe_profile_environment_entry(key: &str, value: &str) -> bool {
    if !safe_environment_entry(key, value)
        || key.len() > MAX_PROFILE_ENVIRONMENT_KEY_BYTES
        || value.len() > MAX_PROFILE_ENVIRONMENT_VALUE_BYTES
    {
        return false;
    }
    let upper = key.to_uppercase();
    if upper.starts_with("PTRACK_") {
        return false;
    }
    ![
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "PASSWD",
        "API_KEY",
        "APIKEY",
        "PRIVATE_KEY",
        "PRIVATEKEY",
        "ACCESS_KEY",
        "SESSION_KEY",
        "CREDENTIAL",
    ]
    .iter()
    .any(|marker| upper.contains(marker))
}

fn is_stable_name(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn contains_nul(value: &str) -> bool {
    value.contains('\0')
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let _ = normalized.pop();
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
}

fn look_path(name: &str) -> io::Result<PathBuf> {
    let path = Path::new(name);
    if path.components().count() > 1 || path.is_absolute() {
        return executable_path(path);
    }
    let search = env::var_os("PATH")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "PATH is not set"))?;
    for directory in env::split_paths(&search) {
        let candidate = directory.join(name);
        if let Ok(path) = executable_path(&candidate) {
            return Ok(path);
        }
        #[cfg(windows)]
        for extension in env::var_os("PATHEXT")
            .and_then(|value| value.to_str().map(str::to_owned))
            .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".to_owned())
            .split(';')
        {
            if let Ok(path) =
                executable_path(&candidate.with_extension(extension.trim_start_matches('.')))
            {
                return Ok(path);
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!("executable {name:?} not found"),
    ))
}

fn executable_path(path: &Path) -> io::Result<PathBuf> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "executable is not a file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "file is not executable",
            ));
        }
    }
    Ok(path.to_owned())
}

fn directory_services_user_shell() -> io::Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let username_output = Command::new("id").arg("-un").output()?;
        if !username_output.status.success() {
            return Err(io::Error::other("current account lookup failed"));
        }
        let username = String::from_utf8_lossy(&username_output.stdout);
        let username = username.trim();
        if username.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "current account name is empty",
            ));
        }
        let output = Command::new("dscl")
            .args([".", "-read", &format!("/Users/{username}"), "UserShell"])
            .output()?;
        if !output.status.success() {
            return Err(io::Error::other("Directory Services lookup failed"));
        }
        let output = String::from_utf8_lossy(&output.stdout);
        let shell = output
            .trim()
            .strip_prefix("UserShell: ")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "UserShell not present in Directory Services record",
                )
            })?;
        Ok(PathBuf::from(shell))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = Command::new("dscl");
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Directory Services is unavailable",
        ))
    }
}
