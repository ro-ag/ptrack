use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::profile::{Profile, ProfileKind, sort_profiles, validate_profile};

pub const PROFILE_CONFIG_VERSION: u32 = 1;
pub const MAX_CONFIGURED_PROFILES: usize = 64;
const MAX_PROFILE_CONFIG_JSON_BYTES: usize = 256 * 1_024;
static TEMPORARY_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileConfig {
    pub version: u32,
    #[serde(default)]
    pub profiles: Vec<Profile>,
}

#[derive(Debug)]
pub enum ProfileConfigError {
    Invalid(String),
    Io {
        context: &'static str,
        source: io::Error,
    },
    Json(serde_json::Error),
}

impl ProfileConfigError {
    fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }

    fn io(context: &'static str, source: io::Error) -> Self {
        Self::Io { context, source }
    }

    /// Reports whether this is the missing-file result accepted at open time.
    #[must_use]
    pub fn is_not_found(&self) -> bool {
        matches!(
            self,
            Self::Io { source, .. } if source.kind() == io::ErrorKind::NotFound
        )
    }
}

impl fmt::Display for ProfileConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => formatter.write_str(message),
            Self::Io { context, source } => write!(formatter, "{context}: {source}"),
            Self::Json(error) => write!(formatter, "decode terminal profile config: {error}"),
        }
    }
}

impl std::error::Error for ProfileConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Invalid(_) => None,
            Self::Io { source, .. } => Some(source),
            Self::Json(error) => Some(error),
        }
    }
}

/// Returns the global terminal profile configuration path beneath `global_home`.
#[must_use]
pub fn profile_config_path(global_home: &Path) -> PathBuf {
    global_home.join("terminal-profiles.json")
}

/// Returns the terminal profile path beneath `PTRACK_HOME` or `~/.ptrack`.
///
/// # Errors
///
/// Returns an error when neither the configured nor platform user home is
/// available.
pub fn default_profile_config_path() -> Result<PathBuf, ProfileConfigError> {
    if let Some(home) = env::var_os("PTRACK_HOME").filter(|value| !value.is_empty()) {
        return Ok(profile_config_path(Path::new(&home)));
    }
    let home = env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .ok_or_else(|| ProfileConfigError::invalid("resolve user home for terminal profiles"))?;
    Ok(profile_config_path(&Path::new(&home).join(".ptrack")))
}

/// Validates, normalizes, and deep-copies a persisted profile configuration.
///
/// # Errors
///
/// Returns an error for an unsupported version, excessive or duplicate
/// profiles, or a profile that fails full validation.
pub fn validate_profile_config(
    config: &ProfileConfig,
) -> Result<ProfileConfig, ProfileConfigError> {
    if config.version != PROFILE_CONFIG_VERSION {
        return Err(ProfileConfigError::invalid(format!(
            "unsupported terminal profile config version {}",
            config.version
        )));
    }
    if config.profiles.len() > MAX_CONFIGURED_PROFILES {
        return Err(ProfileConfigError::invalid(
            "terminal profile config has too many profiles",
        ));
    }
    let profiles = normalize_profile_set(&config.profiles, "configured")?;
    Ok(ProfileConfig {
        version: PROFILE_CONFIG_VERSION,
        profiles,
    })
}

/// Merges configured presentation overrides and custom shells into discovery.
///
/// # Errors
///
/// Returns an error for invalid or duplicate profiles, custom agents, identity
/// repurposing, or a change to a discovered agent's launch identity.
pub fn merge_profiles(
    discovered: &[Profile],
    configured: &[Profile],
) -> Result<Vec<Profile>, ProfileConfigError> {
    let builtins = normalize_profile_set(discovered, "discovered")?;
    let overrides = normalize_profile_set(configured, "configured")?;
    if overrides.len() > MAX_CONFIGURED_PROFILES {
        return Err(ProfileConfigError::invalid(
            "terminal profile config has too many profiles",
        ));
    }

    let mut by_id = BTreeMap::<String, Profile>::new();
    for profile in builtins {
        by_id.insert(profile.id.clone(), profile);
    }
    for profile in overrides {
        let Some(existing) = by_id.get(&profile.id) else {
            if profile.kind == ProfileKind::Agent {
                return Err(ProfileConfigError::invalid(format!(
                    "configured custom agent profile {:?} is not allowed",
                    profile.id
                )));
            }
            by_id.insert(profile.id.clone(), profile);
            continue;
        };
        if profile.kind != existing.kind {
            return Err(ProfileConfigError::invalid(format!(
                "configured terminal profile {:?} changes discovered kind",
                profile.id
            )));
        }
        if profile.provider != existing.provider {
            return Err(ProfileConfigError::invalid(format!(
                "configured terminal profile {:?} changes discovered provider",
                profile.id
            )));
        }
        if profile.kind == ProfileKind::Agent && !same_agent_launch_identity(&profile, existing) {
            return Err(ProfileConfigError::invalid(format!(
                "configured agent profile {:?} changes discovered launch identity",
                profile.id
            )));
        }
        by_id.insert(profile.id.clone(), profile);
    }

    let mut merged = by_id.into_values().collect::<Vec<_>>();
    sort_profiles(&mut merged);
    Ok(merged)
}

fn same_agent_launch_identity(configured: &Profile, discovered: &Profile) -> bool {
    configured.executable == discovered.executable
        && configured.args == discovered.args
        && configured.env == discovered.env
        && configured.cwd_policy == discovered.cwd_policy
        && configured.fixed_cwd == discovered.fixed_cwd
}

fn normalize_profile_set(
    profiles: &[Profile],
    label: &str,
) -> Result<Vec<Profile>, ProfileConfigError> {
    let mut normalized = Vec::with_capacity(profiles.len());
    let mut seen = BTreeSet::<String>::new();
    for source in profiles {
        if seen.contains(&source.id) {
            return Err(ProfileConfigError::invalid(format!(
                "duplicate {label} terminal profile ID {:?}",
                source.id
            )));
        }
        let profile = validate_profile(source).map_err(|error| {
            ProfileConfigError::invalid(format!(
                "validate {label} terminal profile {:?}: {error}",
                source.id
            ))
        })?;
        seen.insert(profile.id.clone());
        normalized.push(profile);
    }
    Ok(normalized)
}

/// Loads one bounded, strictly decoded profile configuration.
///
/// # Errors
///
/// Returns an error for file I/O, empty or oversized input, malformed or
/// trailing JSON, unknown fields, or invalid configuration content.
pub fn load_profile_config(path: &Path) -> Result<ProfileConfig, ProfileConfigError> {
    if path.as_os_str().is_empty() {
        return Err(ProfileConfigError::invalid(
            "terminal profile config path is required",
        ));
    }
    let mut file = File::open(path)
        .map_err(|error| ProfileConfigError::io("open terminal profile config", error))?;
    let mut contents = Vec::with_capacity(4_096);
    Read::by_ref(&mut file)
        .take((MAX_PROFILE_CONFIG_JSON_BYTES + 1) as u64)
        .read_to_end(&mut contents)
        .map_err(|error| ProfileConfigError::io("read terminal profile config", error))?;
    if contents.is_empty() {
        return Err(ProfileConfigError::invalid(
            "terminal profile config is empty",
        ));
    }
    if contents.len() > MAX_PROFILE_CONFIG_JSON_BYTES {
        return Err(ProfileConfigError::invalid(
            "terminal profile config is too large",
        ));
    }
    let config =
        serde_json::from_slice::<ProfileConfig>(&contents).map_err(ProfileConfigError::Json)?;
    validate_profile_config(&config)
}

/// Loads profile configuration, mapping only a missing file to `None`.
///
/// # Errors
///
/// Returns every non-missing I/O, decoding, and validation error unchanged.
pub fn load_profile_config_if_exists(
    path: &Path,
) -> Result<Option<ProfileConfig>, ProfileConfigError> {
    match load_profile_config(path) {
        Ok(config) => Ok(Some(config)),
        Err(error) if error.is_not_found() => Ok(None),
        Err(error) => Err(error),
    }
}

/// Atomically publishes normalized private profile configuration.
///
/// # Errors
///
/// Returns an error when validation, encoding, private temporary publication,
/// permission hardening, or durable synchronization fails.
pub fn save_profile_config(path: &Path, config: &ProfileConfig) -> Result<(), ProfileConfigError> {
    if path.as_os_str().is_empty() {
        return Err(ProfileConfigError::invalid(
            "terminal profile config path is required",
        ));
    }
    let normalized = validate_profile_config(config)?;
    let mut contents = serde_json::to_vec_pretty(&normalized).map_err(|error| {
        ProfileConfigError::invalid(format!("encode terminal profile config: {error}"))
    })?;
    contents.push(b'\n');
    if contents.len() > MAX_PROFILE_CONFIG_JSON_BYTES {
        return Err(ProfileConfigError::invalid(
            "terminal profile config is too large",
        ));
    }

    let directory = path.parent().unwrap_or_else(|| Path::new("."));
    #[cfg(unix)]
    let directory_existed = directory.exists();
    fs::create_dir_all(directory).map_err(|error| {
        ProfileConfigError::io("create terminal profile config directory", error)
    })?;
    #[cfg(unix)]
    if !directory_existed {
        set_unix_mode(directory, 0o700).map_err(|error| {
            ProfileConfigError::io("secure terminal profile config directory", error)
        })?;
    }

    let (temporary_path, mut temporary) = create_private_temporary(directory)?;
    let write_result = (|| {
        temporary
            .write_all(&contents)
            .and_then(|()| temporary.sync_all())
            .map_err(|error| ProfileConfigError::io("write terminal profile config", error))?;
        drop(temporary);
        replace_profile_config(&temporary_path, path)
            .map_err(|error| ProfileConfigError::io("publish terminal profile config", error))?;
        #[cfg(unix)]
        set_unix_mode(path, 0o600)
            .map_err(|error| ProfileConfigError::io("secure terminal profile config", error))?;
        #[cfg(unix)]
        sync_profile_config_directory(directory).map_err(|error| {
            ProfileConfigError::io("sync terminal profile config directory", error)
        })?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    write_result
}

fn create_private_temporary(directory: &Path) -> Result<(PathBuf, File), ProfileConfigError> {
    for _ in 0..128 {
        let sequence = TEMPORARY_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = directory.join(format!(
            ".terminal-profiles-{}-{sequence}",
            std::process::id()
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(file) => {
                #[cfg(unix)]
                set_unix_mode(&path, 0o600).map_err(|error| {
                    ProfileConfigError::io("secure terminal profile config temporary file", error)
                })?;
                return Ok((path, file));
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(ProfileConfigError::io(
                    "create terminal profile config temporary file",
                    error,
                ));
            }
        }
    }
    Err(ProfileConfigError::invalid(
        "create terminal profile config temporary file: name collisions",
    ))
}

#[cfg(unix)]
fn set_unix_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(windows))]
fn replace_profile_config(temporary: &Path, path: &Path) -> io::Result<()> {
    fs::rename(temporary, path)
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn replace_profile_config(temporary: &Path, path: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let from = temporary
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let to = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    // SAFETY: both UTF-16 buffers are NUL-terminated and remain alive for the
    // duration of this synchronous call.
    if unsafe {
        MoveFileExW(
            from.as_ptr(),
            to.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(unix)]
fn sync_profile_config_directory(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}
